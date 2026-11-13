---
schema: design/v1
status: Proposed
problem: |
  koto writes one `koto-<session-id>.json` entry per session into the Claude
  Code session's `workflows` directory, beside Claude Code's own files, and
  nothing ever lists or deletes them. Finished entries pile up without bound,
  and a session whose driver was killed keeps the `running` it last wrote,
  because koto has no daemon, heartbeat or process id for a workflow session.
  The fix has to run from a command-line tool that only acts when called, and
  must never touch a file it didn't write.
decision: |
  Every entry write ends with a sweep of its directory. It lists the directory
  once, removes `.koto-wf-*.tmp` files older than 60 seconds, and stops there
  when at most ten name-matching files exist and none is older than two hours.
  Otherwise it reads only name-matching files, keeps those carrying koto's
  marker, rewrites any still `running` and untouched for over two hours as
  `killed` with "no activity since <time>" and `koto.staleSince`, and removes
  finished entries oldest `startTime` first until ten remain. Live entries
  and the one just written are never removed. Contract version 6; the
  conventions guard gains status-word probes.
rationale: |
  A count is the only rule that bounds a busy directory, and the only place
  that knows the directory and runs often enough is the write itself. The two
  hours is the dashboard's own stalled threshold, so koto has one definition
  of "went quiet", and `killed` is the screen word that reads "stopped" and
  leaves the running count without colliding with real failures. Documenting
  the stale entry as a limitation would leave the running count wrong. Name
  plus content marker fails closed, which is what a shared directory needs.
upstream: docs/prds/PRD-native-workflows-retention.md
decision_provenance: inline-resolved
---

# DESIGN: retention and crash-staleness for koto's `/workflows` entries

## Status

Proposed

## Context and Problem Statement

koto writes one `koto-<session-id>.json` file per koto session into the
viewing Claude Code session's directory,
`~/.claude/projects/<project-slug>/<claude-session-id>/workflows/`, which
Claude Code's own `wf_*.json` files share. The only writer is
`try_materialize` in `src/workflows_surface/materialize.rs`, reached from
`materialize_after_commit` on every committed event
(`LocalBackend::append_event`, `src/session/local.rs:257`). It resolves the
directory, derives the projection, keeps the file's first `startTime`
(`stable_start_time`), and replaces the whole file through `atomic_write`: a
`.koto-wf-*.tmp` temp file in the same directory, `sync_all`, then a rename.
Errors are swallowed with a warning, so a failed write never fails the commit.

Nothing ever lists or deletes files in that directory. `koto session
cleanup`, `koto session prune`, `koto workspace prune` and the terminal
cleanup in `finish_terminal_tick` all work on koto's own store. Finished
sessions' files therefore stay forever, and a session whose driver was killed
keeps the `running` it last wrote. koto has no process id, heartbeat or lease
for a workflow session, and no koto process exists between commands; the only
liveness signal anywhere in koto is the age of a session's last event, which
the dashboard's classifier (`src/cli/dashboard_data.rs`, module `liveness`)
turns into active (under 5 minutes), stalled (2 hours or more) and abandoned
(7 days).

The technical problem is to bound and correct a directory koto shares with
another program, from inside a command-line tool that only runs when someone
calls it, without ever touching a file it didn't write. The PRD fixes what
that has to deliver:

- **A count bound per directory (R1, R2).** After any koto write into a
  directory, it holds at most N owned entries, whatever their status words,
  with a stated default for N. Each directory is bounded on its own.
- **Live entries kept (R3).** The entry being written is never removed by its
  own write, and no live entry is removed to make room; if the live entries
  plus the one just written exceed N, the directory may hold more until they
  finish.
- **Oldest first (R4).** Only finished and stale entries are removal
  candidates, compared by `startTime`, else by the file's modification time,
  ties broken by filename in ascending byte order.
- **Ownership (R5).** Only regular files whose name matches koto's pattern
  and whose content carries koto's marker are removed or rewritten; leftover
  `.koto-wf-*.tmp` files older than an age A are removed too. Everything else
  stays byte-identical.
- **Crash-staleness (R6).** Either a stale entry (still `running`, last
  written more than a window W ago) is rewritten by the next koto write into
  its directory so it no longer reads as running, with its `summary` saying
  when it went quiet, and its own next commit restores it; or the limitation
  is documented with a way to tell live from stale. The design picks one.
- **Never fail, bounded cost, concurrency (R7, R11, R12).** The policy never
  changes the triggering command's outcome, lists the directory once per
  write, reads no file contents when the directory is within N and nothing
  is older than W, and two concurrent koto processes never remove a live or
  protected file or replace a newer write.
- **Guard and docs (R9, R10).** A new top-level status word or field lands
  with Claude Code's conventions guard extended to prove the recorded release
  knows it; the operator guide, field reference and koto-user skill state the
  policy.
- **Tool independence (R8, R13).** koto alone, testable against a temporary
  directory with a controlled clock and no Claude Code or niwa.

The work closes koto issue #190, whose five acceptance criteria these
requirements cover.

## Decision Drivers

- The bound must hold across any number of runs, so it has to be a count.
- koto has no daemon and the screen reads files once on open, so every
  change to the directory happens during some koto write into it.
- A false removal of a file koto didn't write is the one failure that can't
  be undone; ownership checks fail closed.
- The write path runs on every committed step: the common case (a directory
  within the bound) must cost one directory listing and nothing else.
- Whatever status word koto writes must be one Claude Code's screen already
  knows, and provably so: the conventions guard has no status-value probe
  today.
- koto should have one definition of "went quiet", shared by the dashboard
  and the file.
- Reuse what exists: the best-effort envelope around `try_materialize`,
  `atomic_write`, the 60-second temp-file age in `src/cli/batch.rs`, and the
  dashboard's thresholds.
- No new setting unless an operator needs one; a fixed value can become a
  setting later without changing the file.

## Considered Options

### Decision 1: Where the retention policy runs, the bound, and the eviction order

Resolved inline.

The policy needs a host that runs often enough to keep the bound and that
already knows the directory. The directory is known only inside
`try_materialize`: koto's cleanup and prune commands don't know which Claude
Code directory a session wrote to, and the publish-location key that names it
goes away with the session. The bound itself has to be a number an operator
can check by counting files, and the order has to match what the screen
shows, which sorts by `startTime`, newest first.

Key assumptions:

- Claude Code documents no cap or paging for its list, so the bound is about
  legibility, not a screen limit.
- Decision 2's window W decides when a `running` entry is stale.

#### Chosen: a sweep after every entry write, N = 10, fixed

A new `retention::apply` runs at the end of `try_materialize`, right after
`atomic_write` succeeds, with the directory, the path just written and the
current time. Running after the write means the entry it protects is already
the newest on disk, and it inherits the existing best-effort envelope: its
errors never reach the commit. N is the constant `retention::KEEP = 10`. Ten
koto rows next to Claude Code's own entries fit one terminal screen, and the
full history is still in `koto dashboard` and `koto session list`, so a
larger default buys little. It is a constant, not a setting; `apply` takes
its limits as an argument so tests can run with N = 1 or N = 3.

Candidates are finished owned entries (top-level `status` other than
`running`, which after Decision 2 includes reconciled ones). The sweep sorts
them by `startTime` when it is a number, otherwise by the file's
modification time in milliseconds, then by filename in ascending byte order,
and removes from the oldest end until the directory holds N owned entries or
no candidate is left. Live entries and the entry just written are never
candidates.

#### Alternatives Considered

**An age cutoff instead of a count**: remove finished entries older than,
say, 7 days. Rejected because a burst of sessions inside the window still
piles up without bound, which is the operator's actual complaint.

**A `workflows.keep` setting**: let the operator choose N. Rejected for now:
nobody has asked for it, it adds a config key, an environment override and
validation, and it can be added later without touching the file shape.

**Run the sweep from `koto session cleanup`, `prune` or the terminal tick**:
Rejected because those don't know the Claude Code directory, run rarely, and
miss the first write after an upgrade.

### Decision 2: Reconcile a stale entry, or accept it as a limitation

Resolved inline.

A killed session's entry keeps `running`. koto can't know a driver died; it
can only see that the entry hasn't been written for a while. The dashboard
already turns that into a liveness class: a session idle 2 hours or more is
"stalled", and the dashboard shows it that way. The question is whether the
file should say the same, and in which word. Claude Code 2.1.314 knows six
statuses (`pending`, `running`, `completed`, `failed`, `killed`, `paused`),
counts everything but `running` in its "completed" bucket, and gives `killed`
the header word "stopped", `failed` "failed" and `paused` "paused". On a run
that isn't `running`, a step still in `progress` is drawn "Stopped" with the
Outcome "The workflow stopped before this agent finished."

Key assumptions:

- The recorded Claude Code release (2.1.316) still knows `killed`, in a part
  of its bundle a probe can anchor on. Phase 1 of the implementation proves
  this before anything else lands.
- In a working Claude Code session, some later koto write into the same
  directory usually follows a dead session, since koto sessions there come in
  runs.

#### Chosen: reconcile at the next koto write, after the dashboard's 2-hour threshold, as `killed`

When the sweep finds an owned entry whose top-level `status` is `running` and
whose modification time is more than W ago, it rewrites it. W is the
dashboard's `STALLED_THRESHOLD`, 2 hours; the retention module keeps its own
constant `STALE_AFTER` and a unit test pins it equal to the dashboard's, so
the two can't drift apart. A last write exactly W ago, or in the future, is
live.

The rewrite edits the parsed JSON and changes three things:

- the top-level `status` becomes `killed`, which the screen heads "stopped",
  counts out of the running total, and whose stale `progress` step it already
  draws "Stopped";
- the `summary` gains the suffix ` · no activity since <time>`, with the last
  write as UTC to the minute, `YYYY-MM-DD HH:MM UTC`;
- the `koto` block gains `staleSince`, the last write in milliseconds since
  the epoch, so a reader of the file can tell a reconciled entry from one
  koto's projection wrote.

`koto.status` keeps the last classification koto derived from the session's
log (`running` or `blocked`), since the log hasn't changed. This is contract
version 6: `staleSince` is new in the `koto` block and `killed` is a new
top-level word. The reconciled entry is then a finished entry for Decision 1.
When the session commits again, `try_materialize` rewrites the whole file
from its projection, which has no `staleSince` and says `running`.

Reconciliation re-reads the entry's modification time immediately before its
rename and leaves the entry alone if it changed. The remaining window between
that check and the rename is a few system calls; if the owning session's
commit lands inside it, the entry reads `killed` until that session's next
commit, which the docs state.

A session that is alive but quiet for 2 hours, such as one waiting on a human
gate, reads "stopped · no activity since ..." until its next commit. That is
what the dashboard already says about it, and the summary says exactly what
koto knows.

#### Alternatives Considered

**Accept as a documented limitation**: leave `running`, document that
`koto dashboard --once` shows the real liveness, and let the bound age the
entry out. Rejected because it leaves the screen's running count wrong, which
is half of the problem, and the bound only removes an entry once ten newer
ones have finished.

**Reconcile to `failed`**: the screen draws it red with "failed". Rejected
because koto already writes `failed` for real failures and cancellations, so
a dead driver would be indistinguishable from a session that failed.

**Reconcile to `paused` or an unknown word such as `stale`**: `paused` reads
as resumable, and an unknown word gets no icon or header word and no probe
can prove the screen handles it. Rejected.

**A 7-day window (the dashboard's abandoned threshold)**: Rejected because an
operator who kills a session and checks back the same day would still see it
running, which is the case the feature exists for.

**Exempt gate-blocked entries from the window**: Rejected because a session
killed while blocked would then read running forever, and the summary already
says how long it has been quiet.

### Decision 3: Ownership, the temp-file age, and behavior under errors and concurrency

Resolved inline.

The directory is shared, so every removal and rewrite needs proof that koto
wrote the file. koto's session ids aren't guaranteed to be UUIDs (tests use
names like `abc-uuid`), so a filename regex alone proves little. Several koto
processes can work in one directory at once with no lock between them, and
Claude Code may be reading it.

Key assumptions:

- Every contract version koto has written carries `id`, `koto.sessionId`,
  `koto.contractVersion` and a string top-level `status`; Phase 2's tests pin
  this with a version-1-shaped fixture.

#### Chosen: name plus marker, fail closed; 60-second temp age; best-effort steps with re-checks

A file is owned when all of these hold:

- `symlink_metadata` says it is a regular file, not a symlink or directory;
- its name is `koto-<id>.json` where `<id>` passes the session-name character
  check (`validate_session_id`) and contains no path separator;
- opened with no-follow and non-blocking flags, the opened handle is a
  regular file (checked with `fstat` on the handle, not the earlier listing),
  so a file swapped for a link, a pipe or a device between listing and open
  is neither followed nor able to hang the commit;
- a bounded read of at most 1 MiB parses as a JSON object with `id` equal to
  `koto-<id>`, `koto.sessionId` equal to `<id>`, an unsigned integer
  `koto.contractVersion`, and a string top-level `status`.

Anything that fails a check is left exactly as it was and doesn't count
toward N. A `.koto-wf-*.tmp` file is removed when its modification time is
more than A = 60 seconds ago, the same age `sweep_stale_tempfiles` in
`src/cli/batch.rs` uses for koto's other temp files (a unit test pins them
equal). No other file is opened.

Each removal and rewrite is its own best-effort step. Before removing a
candidate the sweep re-reads its modification time and skips it if it
changed, so a file a live session just rewrote is never removed. `NotFound`
from a removal is success, any other error skips that file, and an
unlistable directory ends the sweep. Rewrites use the same temp-file-and-rename
path as `atomic_write`, so a reader never sees half a file and a rename
replaces a path rather than writing through it; the target is never opened
for writing. Warnings name the file and the error kind, never file contents.

#### Alternatives Considered

**Filename only**: Rejected because a person or another tool can create
`koto-notes.json`, and R5 says it must survive.

**A registry of files koto wrote, kept in koto's store**: Rejected because it
is a second source of truth that drifts, a crash between the write and the
registry update loses entries, and the file's own marker already proves
authorship.

**An advisory lock on the directory**: Rejected because Claude Code doesn't
take it, koto's writes are already atomic, and every step here is safe to
repeat or skip.

## Decision Outcome

Every koto write into a `workflows` directory ends with a sweep of that
directory. The sweep lists the directory once. It removes `.koto-wf-*.tmp`
files older than 60 seconds. If no more than ten files match koto's entry
name and none of them was last modified more than 2 hours ago, it stops
there, having read nothing. Otherwise it opens each name-matching file other
than the one just written, keeps those that pass the ownership check, and
classifies each as finished (status not `running`), stale (`running`, last
modified more than 2 hours ago) or live.

Stale entries are reconciled first: each is rewritten as `killed`, with
` · no activity since <time>` on its summary and `staleSince` in its `koto`
block, after a re-check that it hasn't been written meanwhile. It is then a
finished entry. If the directory holds more than ten owned entries, finished
ones are removed oldest `startTime` first until ten remain or no finished
entry is left; live entries and the entry just written are never removed.
When a reconciled session commits again, its next write restores `running`.

The file contract moves to version 6, and the conventions guard learns to
check status words: its coverage test collects top-level `status` values from
the shape fixtures, the conventions fixture gains probes for `running`,
`completed`, `failed` and `killed`, and a second shape fixture holds a
reconciled entry. Nothing here needs a setting, a new command, niwa or the
koto-skills hook. The implementation pull request closes koto issue #190.

## Solution Architecture

### Overview

A new module, `src/workflows_surface/retention.rs`, owns the policy. It is a
pure function of a directory, the path just written, a clock value and a set
of limits, so tests drive it directly with a temporary directory and a fixed
`now`. `try_materialize` calls it as its last step.

### Components

- **`retention.rs` (new).**
  - `pub const KEEP: usize = 10;`
  - `pub const STALE_AFTER: Duration` = 2 hours, pinned equal to
    `cli::dashboard_data::liveness::STALLED_THRESHOLD` by a test.
  - `pub const TEMP_MIN_AGE: Duration` = 60 seconds, pinned equal to
    `batch.rs`'s `TEMPFILE_SWEEP_MIN_AGE` by a test (the constant becomes
    `pub(crate)`).
  - `pub struct Limits { keep: usize, stale_after: Duration, temp_min_age:
    Duration }` with `Default` built from the three constants.
  - `pub fn apply(dir: &Path, written: &Path, now: SystemTime, limits:
    &Limits) -> Sweep`, where `Sweep` reports counts (removed, reconciled,
    temp files removed, files read) for tests and never an error.
  - `fn owned(path) -> Option<OwnedEntry>`: the ownership check, returning
    the parsed object, `startTime`, status and modification time. The listing
    uses `DirEntry::file_type` (no link following); the open uses
    `O_NOFOLLOW | O_NONBLOCK` through `OpenOptionsExt::custom_flags`, then
    `fstat`s the handle and reads through `take(1 MiB + 1)`.
  - `fn reconcile(entry, now) -> bool`: the re-check, edit and atomic rename.
  - a test-only read counter, like `MATERIALIZE_RUNS`, that `apply`
    increments per file opened.
- **`materialize.rs` (changed).** After `atomic_write(&dir, &target,
  &bytes)?`, call `retention::apply(&dir, &target, SystemTime::now(),
  &Limits::default())`. `atomic_write`'s temp-and-rename body is factored into
  a helper the reconcile path also uses.
- **`contract.rs` (changed).** `CONTRACT_VERSION = 6` with a history line;
  `ScreenStatus` gains `Killed`, serialized `killed`; `KotoBlock` gains
  `stale_since: Option<u64>`, serialized `staleSince` and skipped when
  absent. koto's own projection never sets either; they document what the
  reconcile path writes and let a fixture be generated from the types.
- **Guard (changed).** `tests/fixtures/native-workflows/claude-code-conventions.json`
  gains four probes, items `running`, `completed`, `failed` and `killed`,
  each anchored on the part of the recorded release that maps a status to its
  icon and header word (Phase 1 finds the anchor). `tests/native_workflows_conventions.rs`
  also collects the string value of every top-level `status` across the shape
  fixtures and fails naming any with no probe.
  `tests/fixtures/native-workflows/reconciled-shape.json` (new) holds a
  reconciled entry, and the shape test pins it like `enriched-shape.json`.
- **Docs (changed).** `docs/guides/native-workflows-verification.md` gains a
  "Retention and stale entries" section; `docs/reference/session-feed.md`'s
  "Native `/workflows` File" section documents contract version 6, the
  retention rule and the reconciled shape; the koto-user skill's
  `/workflows` section (`plugins/koto-skills/skills/koto-user/SKILL.md`)
  states the same in a paragraph. No setting is added, so
  `docs/guides/cli-usage.md` is unchanged.

### Key Interfaces

The reconciled entry differs from what koto's projection writes in three
fields:

```json
{
  "status": "killed",
  "summary": "koto · work-on · implement · no activity since 2026-11-13 01:58 UTC",
  "koto": { "status": "running", "staleSince": 1794535080000, "contractVersion": 6 }
}
```

`koto.contractVersion` is set to 6 on reconcile, since the entry now carries
a version-6 field. Every other field is kept as it was.

The ownership marker is `id == "koto-" + koto.sessionId`, `koto.sessionId ==
<filename id>`, numeric `koto.contractVersion`, string `status`.

### Data Flow

1. A koto command commits an event; `append_event` calls
   `materialize_after_commit`; `try_materialize` writes the session's entry.
2. `retention::apply` lists the directory once and splits the listing into
   name-matching entries (with `symlink_metadata`) and temp files.
3. Temp files older than A are removed.
4. Fast path: at most N name-matching files and none modified more than W
   ago, so it returns.
5. Otherwise each name-matching file except the one just written is read and
   checked for ownership; failures are dropped from consideration.
6. Stale entries are re-checked and reconciled.
7. If owned entries (the written one included) exceed N, finished ones are
   sorted and removed from the oldest end, each after a modification-time
   re-check, until N remain or none is left.
8. The command returns with its normal result; any error in steps 2 to 7 was
   skipped.

## Implementation Approach

### Phase 1: Prove `killed` in the recorded release and extend the guard

Search the recorded Claude Code release bundle for the status-to-header map,
pick an anchor the guard's rules accept, and add the four status probes. Add
the status-value collection to the coverage test and run the guard against
the recorded release, then with `--alter` to show it fails. If no acceptable
anchor proves `killed`, stop and revisit Decision 2 before anything else.

Deliverables:

- `tests/fixtures/native-workflows/claude-code-conventions.json`
- `tests/native_workflows_conventions.rs`

### Phase 2: Retention sweep with ownership and temp files

Add `retention.rs` with the ownership check, the temp-file sweep, the fast
path, classification and eviction, and call it from `try_materialize`. No
reconciliation yet, so no shape change. Unit tests cover the bound, order,
ties, the N = 1 case, live and just-written protection, two directories,
every ownership negative, temp-file ages, a vanished candidate, an
unremovable candidate, an unlistable directory, the read counter, two
interleaved sweeps, and a version-1-shaped entry.

Deliverables:

- `src/workflows_surface/retention.rs`, `src/workflows_surface/mod.rs`,
  `src/workflows_surface/materialize.rs`, `src/cli/batch.rs` (visibility of
  the temp-age constant)
- `tests/native_workflows_retention_test.rs`: drives the koto binary through
  3N sessions into one `KOTO_WORKFLOWS_DIR` and counts files after each

### Phase 3: Reconciliation and contract version 6

Add reconciliation, the contract changes, the reconciled shape fixture and
its shape test, and the window tests (exactly W, one second past, a future
modification time, the restore on the next commit, a commit landing between
judgment and rewrite, a failed rewrite, a reconciled entry evicted as
finished).

Deliverables:

- `src/workflows_surface/retention.rs`, `src/workflows_surface/contract.rs`
- `tests/fixtures/native-workflows/reconciled-shape.json`,
  `tests/native_workflows_shape.rs`

### Phase 4: Docs

Document the policy in the three places R10 names, and add a reproduction
recipe for the bound to the verification guide: the `cargo test` command for
the 3N test, and a manual loop that runs sessions into a scratch
`KOTO_WORKFLOWS_DIR` and counts files.

Deliverables:

- `docs/guides/native-workflows-verification.md`,
  `docs/reference/session-feed.md`,
  `plugins/koto-skills/skills/koto-user/SKILL.md`

## Security Considerations

The sweep reads, deletes and rewrites files in a directory that Claude Code
and the user also write to, so every step treats file names and contents as
untrusted and fails closed. A file is touched only when it is a regular file
(checked without following links), its name is `koto-<id>.json` with a valid
session id and no path separator, and a bounded read of at most 1 MiB parses
to JSON carrying koto's marker: `id` equal to `koto-<id>`, `koto.sessionId`
equal to `<id>`, a numeric `koto.contractVersion` and a string `status`.
Anything that fails a check is left unchanged and ignored. Leftover
`.koto-wf-*.tmp` files are removed only when they are regular files older
than 60 seconds. No path used for a read, removal or rewrite comes from file
contents, the sweep never recurses, and it runs with the invoking user's own
privileges, so it can't delete or overwrite anything outside the one
directory it was handed. Removal uses unlink and rewriting uses a temp file
in the same directory plus a rename; neither follows a symlink at the final
path component.

A check and the action after it are separate system calls, so a file can
change in between. For reads, koto opens the file with no-follow and
non-blocking flags, confirms from the opened handle that it is a regular
file, and reads at most 1 MiB, so a file swapped for a link, a pipe or a
device can't be followed or make a commit hang. For removal and
reconciliation, koto re-reads the modification time just before acting and
skips the file if it changed; a session that commits inside the remaining
window of a few system calls can read `killed` until its next commit, which
the docs state. A local actor who can swap files in the directory can
already edit them directly, so this isn't treated as a privilege boundary.
The directory comes from `KOTO_WORKFLOWS_DIR` or from the Claude Code project
path and is trusted configuration; if it points somewhere unexpected, the
ownership marker still limits the sweep to koto's own entries.

The change adds no dependency and no network access. The weekly conventions
guard reads a downloaded Claude Code release as text and never runs it. The
reconciled entry adds a UTC time and an epoch-millisecond value derived from
the file's own modification time, which the same user could already read
from the file system; no paths, credentials or environment values are
written, and warnings log the file name and error kind, not file contents.

## Consequences

### Positive

- The `/workflows` list stays at ten koto rows plus whatever is live, however
  much koto is used, and the first write after upgrading cleans an existing
  backlog.
- A dead koto session stops counting as running within one koto write after
  2 hours, and says when it went quiet.
- The file and the dashboard share one definition of "went quiet".
- The guard checks status words for the first time, so a Claude Code release
  that drops any of the four fails CI.
- The common case costs one directory listing per write.

### Negative

- A live session quiet for more than 2 hours (a long human gate) reads
  "stopped" until its next commit.
- Nothing changes in a directory until some koto write lands there; a
  directory whose last koto session died keeps its `running` entry.
- Ten is a judgment call, and it's fixed: an operator who wants more history
  in the screen can't have it without a code change.
- A commit that lands in the few system calls between reconciliation's
  re-check and its rename reads `killed` until that session's next commit.
- The file contract gains a version and the guard four probes to maintain.

### Mitigations

- The summary says "no activity since <time>", which is true, and the next
  commit restores the entry; the docs say both.
- The docs point to `koto dashboard --once` for a live view and explain that
  reconciliation needs a later koto write.
- `apply` takes its limits as an argument, so a setting for N is a small,
  shape-free change if operators ask for it.
- The guard's probes run weekly against the newest Claude Code release, so a
  removed status word is caught before an operator sees it.
