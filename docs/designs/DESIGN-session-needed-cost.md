---
schema: design/v1
status: Proposed
problem: |
  `koto session needed PATH` is the removal gate a workspace manager runs
  before it deletes an instance or a worktree, once per candidate. Every call
  reads the whole local sessions directory twice, whatever PATH is: the
  hierarchy graph's scan over every session, then a second pass over every
  header. At about 9,600 sessions a call retires about 22 billion
  instructions and takes 1.65 to 6.55 seconds on a loaded host, so niwa's
  30-second step budget answers only a few candidates per reap pass and
  keeps the rest because time ran out. The cost grows with the store, not
  with the path asked about.
decision: |
  koto keeps a path index in the sessions directory, `.path-index.jsonl`:
  append-only rows mapping each session to its header's execution_dir, kind
  and id, the continues links and imported copies the lineage check
  follows, and a witness of each state log's filesystem identity (device,
  inode, birth time). Every code path that writes an anchor, a continues
  link or an imported-from record appends its row first, and every
  whole-log replacement appends a witness of the file it wrote. `needed`
  lists the store, lstats each state log, and opens headers only for
  sessions the index places under PATH, their lineage neighbours, and any
  session whose identity doesn't match its witness or that holds a cloud
  backend file. A missing, damaged or incomplete index, a header that
  doesn't parse, or no birth time sends it to an exact header-only scan
  with one stderr line saying why. Answers don't change. niwa gains an
  optional once-per-pass `preflight` command and dot-niwa moves its
  `--help` probe into it; niwa's per-path contract and budgets stay.
rationale: |
  Only an index takes header reads off the per-call cost: a narrower scan
  cuts today's work by about five times but still opens every header,
  about ten seconds at ten times today's store, and batching in niwa
  changes niwa's engine-neutral contract and still costs one full scan per
  pass. The identity witness makes the index safe against writers that
  don't know it: a rename-based rewrite gives a state log a new identity,
  so an older koto's rebind into PATH is re-read, while tick appends keep
  the identity and cost nothing. The price is a few lstats per session, an
  estimated 0.1 to 0.3 seconds a call today on a quiet host and 1 to 3 at
  ten times; that meets the 2-second requirement today. Falling back to the
  exact scan rather than answering 2 keeps removals working while the index
  heals.
motivating_context: |
  On niwa main a reap pass has 30 seconds for its mapped units and 30 for
  each gate step, and asks serially. Timed at 9,632 sessions, no `needed`
  call fit twenty candidates into one step, and passes kept instances with
  "time ran out" reasons that had nothing to do with whether they were
  needed.
user_visible_surface: true
---

# DESIGN: Answering koto session needed without a store scan

## Status

Proposed

## Context and Problem Statement

`koto session needed PATH` answers whether any workflow session still needs a
directory (`docs/guides/cli-usage.md`, "session needed"). It exits 0 when
PATH isn't needed, 1 when it is, with one line of at most 200 bytes naming
the session whose name sorts first and why, and 2 when it can't decide.
niwa's removal gate is the main caller. niwa never names koto: it runs a
workspace-declared `[session.before_remove]` shell command before every
removal, reads only the exit status, keeps on anything but 0, and uses the
last non-empty output line as the keep reason (niwa
`internal/cli/removal_gate.go`). The tsukumogami workspace declares that
command as three fail-open guards followed by
`exec koto session needed "$NIWA_REMOVE_PATH"`, with a 10-second timeout
(dot-niwa `.niwa/workspace.toml`). The contract both sides follow is niwa's
`docs/designs/DESIGN-instances-outlive-sessions.md` and its plan: exit 0
removes, anything else spares, every failure spares, and the verb never
answers 0 for a store it can't list.

### What a call costs today

`needed()` (`src/session/needed.rs`) does a full scan, twice over, on every
call:

1. `Graph::scan` (`src/session/hierarchy/graph.rs`) lists the store and runs
   `SessionFacts::read` (`src/session/hierarchy/facts.rs`) for every
   session: it opens the state log and parses its header, reads and parses
   `ctx/manifest.json`, reads the pin, walks `ctx/koto/` to depth 4, opens
   every link file, and checks `ctx/chain/parent`. `find_migrated` reads
   whole logs for imported copies.
2. `needed()` then loops over every name again and opens each state log a
   second time to read `execution_dir` (`read_anchor`), because the graph's
   parsed header doesn't keep it.
3. Only sessions anchored at or under PATH get the expensive read: the whole
   log replayed by `inventory::inspect` (`src/session/inventory.rs`), and for
   running ones the lineage check `held()` and, under the cloud backend, the
   bucket walk (`src/session/needed/bucket.rs`).

The graph is used only for lineage (kind, header id, resolved continues
links, unreadable links and imported copies), and lineage matters only for
the handful of sessions under PATH. Status plays no part in the scan:
running, terminal, cancelled sessions and closed anchors cost the same.
Nothing is cached between calls, and no lock is taken.

Timed on a development host whose store held 9,632 session directories
(67,192 files, 3.5 GiB, 2,938 of them closed anchors), each command run
alone with the store already in memory:

| Call | Answer | real s | user s | sys s | instructions |
|------|--------|--------|--------|-------|--------------|
| `needed`, instance A | needed (4 sessions) | 6.55 | 0.33 | 1.75 | 23.6e9 |
| `needed`, instance B | needed | 1.65 | 0.29 | 1.16 | 22.4e9 |
| `needed`, instance C | not needed | 3.10 | 0.62 | 2.34 | 22.2e9 |
| `needed`, instance D | not needed | 3.79 | 0.61 | 2.89 | 22.4e9 |
| `needed`, instance E | not needed | 3.00 | 0.55 | 2.40 | 22.0e9 |
| `session list --execution-dir`, C | n/a | 1.07 | 0.12 | 0.70 | 4.8e9 |
| `needed`, instance C again | not needed | 5.91 | 0.62 | 3.05 | 22.9e9 |

Every `needed` call did the same 22 to 24 billion instructions whatever the
path or the answer, so a typical call is about 3 seconds and the spread
comes from host load, not the cache (no block input on any run; the repeat
on C was slower than the first). Most of the time is kernel time spent on
about 67,000 file opens and stats. The header-only scan behind
`koto session list --execution-dir` (`src/session/local.rs`) does about a
fifth of the work.

### Why that breaks the gate

niwa asks serially, one candidate at a time, with no batching or cache. Each
run gets the declared 10-second timeout; each gate step has a 30-second
budget (`defaultRemovalGateBudget`); and on niwa main, since niwa#581, the
whole mapped-reap stage has a 30-second deadline (`passDeadline`, niwa
`internal/cli/pass.go`) that also covers rule evaluation, git reads and the
removals. A candidate the budget can't reach is kept with one of three
reasons: `before_remove timed out after 10s`, `this step's 30s before_remove
time ran out before it could ask`, or `the pass's time ran out before
before_remove could ask`. Nothing is persisted, and the next pass asks again.
At 3 to 7 seconds a call, one pass answers four to ten candidates and keeps
the rest for a reason unrelated to whether they're needed. Each ask also
starts two koto processes, since the declaration probes
`koto session needed --help` before the real call.

### Goal

Every `needed` answer in under 2 seconds at today's store size, and twenty
candidate instances answered in one reap pass with no time-ran-out keep.
The verb's answers don't change: same exit codes, same line, same meaning,
whichever koto wrote the store. The gate fails toward keeping, never toward
removing a live instance. The one answer that can differ is for a header
damaged in place by something other than koto, in a session the fast path
has no reason to read; Security Considerations shows why that can't remove
anything live.

## Decision Drivers

- **D1. Same answers.** Exit 0, 1 and 2 and the printed line stay exactly
  as `needed()` computes them today, including its edges: a header the verb
  reads and can't parse makes the answer 2 unless another session needs
  PATH, and a PATH that can't be canonicalized is 2.
- **D2. Cost well inside the budget.** Under 2 seconds per call at today's
  size is the requirement. The design target is under 0.5 seconds at about
  9,600 sessions, so twenty candidates fit one 30-second step with room
  left for the pass's other work, and a cost that grows with the sessions
  under PATH rather than the store wherever that doesn't mean trusting
  writers the index can't see.
- **D3. niwa's contract stays.** One engine-neutral command per path, exit
  status only. Nothing in niwa should need to know koto has an index, and
  anything niwa gains must be engine-neutral too.
- **D4. Fail toward keeping.** Anything the index gets wrong, and anything
  a writer that doesn't know the index does, may cost time or keep an
  instance; it must never let a running session's directory be removed.
- **D5. No silent degradation.** koto#640 is an index that falls back to a
  full sweep and says nothing; koto#637 is a slow-path hint that names a
  command that doesn't help. A fallback here says so in one line and names
  the command that fixes it.
- **D6. The verb writes nothing.** `needed` is documented as read-only and
  niwa's design relies on that. It may read an index; it may not build or
  repair one.
- **D7. The cloud bucket walk is untouched.** koto#501's walk runs for
  running sessions under PATH that the local lineage doesn't release, and
  keeps its limits.
- **D8. No dependency on pruning.** The store keeps growing until
  retention catches up, and store pruning is on hold for now.

## Considered Options

### Decision 1: How `needed` finds the sessions under PATH

The answer depends on three things: which ordinary sessions are anchored at
or under PATH, the lineage of the running ones (continues successors and
imported copies, wherever they're anchored), and whether any session's
header in the store can't be read. Today all three come from reading every
session. The question is what replaces that read.

#### Chosen: a path index kept by koto's writers, checked against each state log's identity (B)

koto keeps an append-only file in the sessions directory with one row per
session (name, header id, kind, execution_dir), one per continues link
(holder, named predecessor, recorded id), one per imported-from record
(copy, source name, source id), and a witness of each state log's
filesystem identity, written after every whole-log replacement. Every code
path that writes one of those facts appends its row first. `needed` reads
the index, lists the store once to confirm the index knows every session,
and lstats each state log to confirm nothing replaced it behind the index's
back. It opens files only for sessions the index places under PATH, the
running ones' lineage neighbours, and any session whose identity no longer
matches its witness, judging each through the same per-session code as
today. The index supplies candidates; the files supply the answer.

The index is roughly 2 to 3 MB today and ten times that at ten times the
store, read sequentially; the lstats are what scale. Calibrated against the
header-only scan (about 1.07 seconds quiet for 9,600 sessions, roughly 110
microseconds each to open, read, parse and close a header), an lstat is a
small fraction of that. The estimate is about 0.1 to 0.3 seconds a call
today on a quiet host, up to about 1 second under load, and about 1 to 3
seconds quiet at ten times the store. The acceptance check below times it.

#### Alternatives considered

**A narrower scan (A).** Drop the graph build: one header read per session
to find the candidates and spot unreadable headers, then read lineage only
when a running session is under PATH, by listing each session's
`koto/links/continues/` and `koto/imported-from`. A finished instance with
no running session needs only the header pass. This is what
`session list --execution-dir` already costs: 4.8e9 instructions and about
1.07 seconds today on a quiet host, two to four times that under load, and
about ten seconds at ten times the store. It needs no new file and can't go
stale. Rejected as the main path because every call opens, reads and parses
every header, several times the chosen option's per-session stat, and
twenty candidates at 1 second already fill two-thirds of a step today. It's
kept as the fallback (Decision 2), where it beats today's scan by about
five times with the same answer.

**Batching in niwa (C).** niwa would collect a pass's candidates and ask
once, and koto would answer every path from one scan. One scan per pass
instead of one per candidate fixes the step budget at today's size. But it
changes niwa's gate from one engine-neutral command per path into a
multi-path protocol every engine would have to speak (D3), each pass still
pays a full scan (1.65 to 6.55 seconds today, 16 to 65 at ten times the
store, past the 30-second deadline), and a single call no longer meets a
2-second bound. A batch contract would need its own niwa design; it isn't
needed once a single call is fast.

**Trust the index without a per-session check.** That would make the fast
path O(sessions under PATH), but a pre-index koto can rebind a session into
PATH and append nothing. A trigger of "any session directory newer than the
index" doesn't help: an aware tick appends to the index and then writes into
its session directory, so it fires on every ordinary write. Rejected while
pre-index koto can write to the store.

### Decision 2: What `needed` answers when it can't trust the index

The index can be missing (a store from before this change, or a deleted
file), damaged (a line that doesn't parse), or incomplete (a session
directory the index has no row for, which means some writer skipped it).
The filesystem may report no birth time, so a witness can't rule out inode
reuse. And a header the fast path reads may not parse. The verb has to
answer something, and the answer must never move toward removal (D4).

#### Chosen: answer from the exact narrow scan, and say so on stderr

`needed` runs option A's header-only scan, which gives the same answer as
today at about a fifth of the cost, and prints one line to stderr before its
answer, for example:

```text
koto: the path index doesn't list 3 sessions in this store, so this answer came from a full scan; run `koto session path-index rebuild` to fix it
```

The line names which check failed and the command that fixes it (D5); on a
filesystem with no birth time it says that instead, since a rebuild can't
fix it. It's printed before the answer because niwa takes the last
non-empty line as the keep reason, so the answer line stays the reason
exactly as today. If the fallback runs past niwa's 10-second timeout at a
much larger store, niwa kills it and keeps the instance, which is the safe
direction. `needed` never writes the index itself (D6); it's rebuilt by the
verb the line names or by the next koto housekeeping pass (Solution
Architecture).

#### Alternatives considered

**Answer 2 while the index is stale.** Cheap and safe, but every removal in
the workspace stops until someone rebuilds, which turns a missing file into
an outage of the reaper. An exact answer at about 1 second today is better
than no answer. Rejected.

**Fall back to today's `Graph::scan` path.** Same answer, five times the
work of option A. Rejected; the graph path stays only as the test oracle.

### Decision 3: What changes on niwa's side

#### Chosen: a once-per-pass preflight in niwa, and dot-niwa moves its probe into it

At an estimated 0.1 to 0.3 seconds a call, twenty candidates fit both the
30-second step budget and the 30-second pass deadline, so niwa's per-path
contract and budgets stay. The `koto session needed --help` probe stays as
a guard but runs once per reap pass. niwa runs the declared command per
candidate with no state between runs, so that needs a small, engine-neutral
addition: an optional `preflight` command in `[session.before_remove]`, run
once per pass and reused for every gate in it, into which dot-niwa moves
its probe (details in the niwa section of Solution Architecture).

#### Alternatives considered

**Drop the probe.** It saves the same process start, but a koto without the
verb would then be judged by clap's usage error instead of by an explicit
guard that says what's missing. Rejected.

**Keep the probe per candidate.** No niwa change, but every ask starts two
koto processes, doubling process starts for a check whose answer can't
change within a pass. Rejected.

**Raise niwa's step budget or pass deadline.** That would make today's slow
calls fit, but every implicit reap at create, dispatch and watch can already
spend about 90 seconds in gate runs (mapped reap, backstop, worktree sweep),
and a longer budget makes that worst case longer for every user while fixing
nothing in koto. Rejected; it can be revisited if the acceptance check shows
time-ran-out keeps with a healthy index.

## Decision Outcome

koto gains a path index, `.path-index.jsonl` in the sessions directory, kept
by the code that writes the facts it records. Each row is one short JSON
line appended the way `_terminal_index.jsonl` appends
(`src/engine/terminal_index.rs`): `O_APPEND`, at most 4 KiB, fsync. A
session row records name, header id, kind and execution_dir; a continues row
records holder, named predecessor and recorded id; an import row records
copy, source name and source id; a witness row records a state log's
device, inode and birth time. Writers append fact rows before the write
they describe, so a crash between the two leaves a row with no matching
write, never a write with no row. A witness row follows the whole-log
replacement it records, and a crash before it leaves a log whose identity
matches no witness, which the reader re-reads. Rows are never edited in
place; a rebuild rewrites the file from a full scan under a lock that
writers hold shared.

`needed` stays one call per path. It reads the index, lists the store once,
lstats every listed state log, and collects candidates from the index plus
every session whose identity doesn't match its witness or that the cloud
backend has touched. It reads each candidate's real header and keeps
today's per-session judgment unchanged. Anything it can't trust sends the
call to the exact header-only scan with one stderr line. Sessions the index
places elsewhere, whose identity matches, are never opened. The read path
below has the steps.

A read-only `koto session path-index status` reports what the index knows
and whether `needed` would trust it, and `koto session path-index rebuild`
rebuilds it. koto's existing housekeeping at the start of `koto next` (where
the terminal index is compacted) also rebuilds an index that's missing,
damaged, incomplete or oversized, best effort, under the same lease
discipline. That's compaction and repair; nothing about safety waits on it.

On niwa's side, `[session.before_remove]` gains an optional `preflight`
command run once per reap pass, and dot-niwa moves its `--help` probe into
it. Pruning old terminal sessions isn't part of this design and isn't
needed for it.

### Rationale

The index changes what the verb reads, not what it decides. Every fact the
answer rests on comes from the session's own files; the only thing taken
on the index's word is that a session it places elsewhere, whose state log
still has the witnessed identity, isn't anchored under PATH. Row-before-write
ordering makes the index a superset of what aware writers did, the listing
catches writers that never wrote a row, the witness catches writers that
replaced a log without one, and a fallback that announces itself covers the
rest. The cost accepted is a second record of each anchor, a witness row
per whole-log replacement, and lstats that keep the call growing with the
store, slowly, until no pre-index koto can write to it.

## Solution Architecture

### Overview

```text
writers (init, adoption, rebind, rename, import, pull, link --continues)
   |  1. append fact row to <sessions>/.path-index.jsonl  (shared lock)
   |  2. write the header / link / imported-from          (as today)
   |  3. after a whole-log replacement: append a witness row
   v
koto session needed PATH
   canonicalize PATH -> read index -> list store names -> lstat state logs
        | trusted                          | missing/damaged/incomplete,
        v                                  | no birth time, bad header read
   candidates from rows, plus             v
   identity mismatches and           stderr notice, exact header-only scan
   cloud-touched sessions                  |
        |                                  |
        +---- per-session judgment (unchanged) ----+--> exit 0 / 1 / 2
```

### The index file

It lives at `<sessions>/.path-index.jsonl`, next to the sessions it
describes, so `KOTO_HOME` and `KOTO_SESSIONS_BASE` move it with the store and
`needed` finds it from the same `needed_store()` it already resolves. A dot
file is never a session: `session_names` skips dot entries and
`LocalBackend::list` skips non-directories. It's local only and never pushed
to a bucket.

Rows, one JSON object per line, each tagged with the format version:

```text
{"v":1,"t":"built","koto":"<version>","at":"<rfc3339>"}
{"v":1,"t":"session","name":"<name>","id":"<id>","kind":"ordinary","anchor":"<dir>"}
{"v":1,"t":"session","name":"<name>","id":"<id>","kind":"ordinary","anchor":null}
{"v":1,"t":"unreadable","name":"<name>"}
{"v":1,"t":"long","name":"<name>"}
{"v":1,"t":"continues","holder":"<name>","peer":"<pred>","peer_id":"<id or null>"}
{"v":1,"t":"import","copy":"<name>","from":"<source>","from_id":"<id>"}
{"v":1,"t":"witness","name":"<name>","dev":<n>,"ino":<n>,"born":"<ns since epoch>"}
```

A `built` row opens every rebuilt file. `unreadable` comes only from a
rebuild that found a header it couldn't parse. A row that wouldn't fit in
4 KiB (an anchor near the platform's path limit) is written as `long`
instead, which tells the reader to open that session's header on every call.
Removals write nothing: a removed session simply stops appearing in the
listing, and its rows are ignored until the next rebuild drops them. The
reader folds fact rows into sets, not latest-wins: a session with three
recorded anchors is a candidate for all three until a rebuild, which keeps
the ordering of concurrent appends from mattering. Witness rows are the one
exception: only a session's newest witness counts.

### Writers

The hooks sit at the primitives every caller funnels through, not at each
verb:

| Fact | Where it's written today | Row |
|------|-------------------------|-----|
| new session directory and header | `LocalBackend::create`, `init_state_file` (`src/session/local.rs`), `persistence::append_header` | session |
| anchor at init | `koto init --execution-dir` (`canonical_dir`, `src/cli/mod.rs`), default from cwd (`init_child::canonical_or_verbatim`), a child copying its parent's (`resolve_execution_dir`, `src/cli/init_child.rs`), anchor and close sessions (`create_kind_session`, `src/session/anchor.rs`) | session |
| anchor adopted on a first tick | `check_execution_anchor` returning `Adopt`, then `record_execution_anchor_adoption` (`src/cli/mod.rs`) through `rewrite_header` | session |
| anchor moved by a rebind | `handle_rebind` (`src/cli/session.rs`) and `move_header` (`src/cli/rebind_whole.rs`), both through `rewrite_header` | session |
| name changed | `rewrite_header_identity_within` (`src/session/local.rs`), used by relocation and `src/session/recover.rs` | session |
| continues link | `links::write_continues` and `write_reserved_record` (`src/session/links.rs`) | continues |
| imported-from record | `src/session/import_links.rs` | import |
| cloud pull materializing a session or replaying reserved keys | the cloud backend's local state-file and context writes (`src/session/cloud.rs`) | session, continues, import |
| any whole-log replacement | `replace_log_bytes` (`src/engine/persistence/append.rs`) | witness |

Every header rewrite goes through `claim::rewrite_header_under_lock`
(`src/engine/claim.rs`), which already holds the new header before it calls
`replace_log_bytes`. The session-row hook goes there: when the new header's
`execution_dir`, `kind`, id or name differs from the old one, append a
session row, then write. Header rewrites that touch only claims or epochs
append no session row.

The witness hook goes in `replace_log_bytes` itself, the one funnel for
whole-log replacement: header rewrites, relocation
(`src/session/local.rs`), and the cloud backend's rename-based pulls all
reach it. It writes the temp file, takes the file's identity with `fstat`
on its own open temp file, renames, and then appends the witness row.
Taking the identity before the rename means a third party's rewrite after
the rename can't be witnessed by mistake. So the row rate follows inits,
re-anchors and whole-log replacements; the event appends every tick makes
keep the file's identity and write no row.

One writer replaces a state log in place today: the cloud backend's
`write_local_state_bytes` (`src/session/cloud.rs`), a plain `fs::write`
used by child reconciliation (`koto session resolve --children`) and by the
supersede fence's adoption of a local predecessor
(`src/session/cloud_supersede.rs`). This release moves it onto
`replace_log_bytes`. The other `fs::write` calls that produce a state log
(the import in `src/session/cloud.rs`, the workspace read in
`src/session/cloud_workspace_read.rs`) write into a staging or scratch
directory.

### Rebuild and compaction

A rebuild runs option A's scan plus a read of every session's continues
links and imported-from record, writes a new file, and renames it over the
old one. For each session it lstats the state log before it reads the
header and records a witness from that stat, so a rewrite between the stat
and the read shows up as a mismatch on the next call: one extra re-read,
never a wrong answer. It runs from `koto session path-index rebuild`, and
from the housekeeping at `koto next` startup when the index is missing,
damaged, lacks a row for a listed session, or has more than twice as many
rows as the store has sessions. Housekeeping checks at most once every ten
minutes, using the lock file's mtime as its clock, so a tick pays one stat
in the common case. It's compaction and repair; the read path is safe
without it.

Writers and a rebuild coordinate through `flock` on
`<sessions>/.path-index.lock`, which the kernel releases if a holder dies.
A writer holds it shared across its append and the write it describes. A
rebuild takes it exclusively twice, for milliseconds each: at the start to
record the file's length, and at the end to copy every row appended past
that length into the new file and rename. A writer that was between its row
and its write when the rebuild started finished before the rebuild got the
lock, so the scan sees its write; a writer that started later appended past
the recorded length, so its row is carried over. A rebuild that can't get
the lock within the log-lock wait gives up and leaves the old file, which is
still a superset.

### The read path

1. Canonicalize PATH; on failure answer 2 as today.
2. Read `.path-index.jsonl`. Missing, a `v` it doesn't know, or any line
   that doesn't parse (except a final line with no newline, which is a
   crashed append and is skipped, as the terminal index does) sends it to
   the fallback.
3. List the store's names with one `read_dir`, applying `session_names`'
   filter on the name only. Any listed name with no row sends it to the
   fallback: some writer skipped the index.
4. For every listed session, lstat its state log (no open) and compare
   device, inode and birth time with its newest witness. If the filesystem
   reports no birth time, inode reuse can't be ruled out, so the call goes
   to the fallback and says why. A mismatch, or no witness, means a write
   the index didn't see: the session becomes a candidate. Also lstat the
   session directory's cloud backend files (`cloud-sync.json` and
   `cloud-check.json`, constants in `src/session/sync_stamp.rs`, and
   `version.json`, which the cloud backend creates before its first push);
   a session holding any of them is always a candidate, never judged from
   its witness.
5. The other candidates: ordinary sessions with any recorded anchor under
   PATH (`under_dir`, whole components), ordinary sessions recorded with no
   anchor, and every `unreadable` and `long` row. Each candidate gets the
   state-file check `session_names` makes, and its real header is read. A
   header that doesn't parse sends the whole call to the fallback, so the
   answer is today's: 2 unless something else needs PATH.
6. Judge each candidate exactly as `needed()` does today. A re-read session
   whose real anchor isn't under PATH drops out like any other. For a
   running one, `held()` looks up continues rows whose `peer` is the
   session's name or whose `peer_id` is its header id, and import rows
   whose `from` is its name, then reads those holders' and copies' own
   facts with `SessionFacts::read` and applies the same resolution
   `build_directed` and `find_migrated` apply. The session rows' ids give
   the id-to-name map that resolution needs.
7. Under the cloud backend, the bucket walk runs for the same sessions as
   today, with the same limits.
8. Print and exit as today.

Why the witness separates the two kinds of writer: every whole-file
replacement of a state log in koto goes through a temp file and a rename.
`replace_log_bytes` does, and `rewrite_header_under_lock` calls it; every
release that has `koto session rebind` rewrote the header through
`rewrite_header_atomically`, temp file plus rename. A rename-based rewrite
always gives the state log a new identity: the temp file exists beside the
old one before the rename, so they can't share an inode, and birth time
rules out a later rewrite reusing a freed one. So an older koto's rebind
into PATH is re-read, an aware writer's replacement is witnessed, and its
appends change nothing. A crash between an aware writer's rename and its
witness row lands on the mismatch side, which is safe.

In-place overwrites keep the identity, which is why cloud-touched sessions
are always re-read. Under `auto` the child reconciliation replaces local
only when local is a strict prefix of remote, so the header line can't
change; under `accept-remote`, and in the fence adoption, it can. The
argument that a session with none of the cloud files was never overwritten
in place by a released koto: the cloud backend creates `version.json`
before every push it attempts (`load_or_create_local_version`), and its
pulls and listings leave `cloud-sync.json` or `cloud-check.json`, so a
session holding none of them was never pushed, pulled or probed by the
cloud backend on this host. The fence adoption stamps after it writes.
Child reconciliation stamps nothing itself and visits only children this
host already lists, so the argument assumes such a child was pushed at
least once before an older koto reconciled it.

The implementation keeps one judgment function and gives it two candidate
sources, the index and the narrow scan. Tests run both, and today's graph
path as an oracle, over the same fixture stores and require identical
answers. A debug-only environment switch makes `needed` compute both answers
and abort when they differ, and the integration suite runs with it on, so a
new write path that forgets its hook fails CI instead of shipping. The
index's own tests go in a new `tests/session_needed_index_test.rs`:

1. `unaware_rebind_into_path_is_seen`: build a fixture store and its index,
   then rewrite one session's header so its execution_dir is under PATH
   with a plain temp-file-and-rename that appends no row, which is what a
   pre-index koto's `koto session rebind` does. `needed PATH` exits 1
   naming that session, matching the scan oracle.
2. Aware ticks keep the fast path: tick sessions outside PATH (event
   appends) and assert `needed` doesn't fall back and opens no header
   outside PATH, through a debug-only read counter.
3. An in-place overwrite of a cloud-synced session's state log that moves
   its anchor into PATH, with the cloud stamp present: `needed` exits 1.
4. A candidate header that doesn't parse sends the call to the scan, with
   the same answer as the oracle.
5. No birth time, through a test switch that hides it: the fast path is
   refused and the answer comes from the scan.

### Staleness, in one rule

The index is trusted only when it parses, its format is known, every
listed session has a row, and the filesystem reports birth times; otherwise
`needed` answers from the exact scan and says so. Inside that rule, an
index that over-reports costs a few header reads, and one that's out of
date for a session it lists can't hide that session from PATH: either an
aware writer appended before writing, or the write changed the log's
identity, or the cloud backend touched it and it's read anyway.

### Cost

| | Today (about 9,600 sessions) | Ten times |
|---|---|---|
| Today's `needed` (graph scan plus header pass) | about 22e9 instructions, 1.65 to 6.55 s | about 220e9, 16 to 65 s |
| Option A, the fallback (header pass) | about 4.8e9, about 1.07 s quiet, 2 to 4 s loaded | about 48e9, about 10 s quiet |
| Option B, the index (estimated) | about 0.1 to 0.3 s quiet, up to about 1 s loaded | about 1 to 3 s quiet |

The fast path is one directory listing, the index parse, a few lstats per
listed session (the state log and the cloud files), and opens only for
candidates and mismatches. The lstats keep it O(store), at a small fraction
of the scan's per-session cost. At today's size it meets the 2-second
requirement with room, and twenty candidates fit the 30-second step: about
2 to 6 seconds quiet, more under load. At ten times the store it no longer
meets the 0.5-second design target. A path with many sessions under it (a
workspace root rather than an instance) costs what those sessions cost, as
it does today.

What would bring the cost back to O(sessions under PATH) is knowing that no
pre-index koto can write to the store, for example a store layout version
that every koto writer honors. Today only the reader crate checks
`.koto-store-layout` (`koto-session-reader/src/source.rs`). Once writers
refuse a store whose layout they don't know, the per-session stats can go.
That's future work, not part of this design.

### Old terminal sessions

The scan doesn't look at status: a session cancelled months ago costs the
same as a running one, and so does a closed anchor, so cancelling abandoned
runs doesn't make today's verb faster. Under the index they cost a few rows
and a few lstats, and are opened only when they're under PATH or their log
changed identity. Pruning would shrink the scan, the index and the stats,
but store pruning is on hold, and this design neither waits for it nor
changes what prune keeps.

### The niwa side

niwa's per-path contract and budgets stay as they are: one command per
path, exit status only, 10 seconds per run, 30 seconds per step, and on
main the 30-second pass deadline.

- **Per-call target.** Twenty candidates in one 30-second step need well
  under 1.5 seconds a call, and less once the pass deadline's git reads and
  removals are counted. The estimate is 0.1 to 0.3 seconds a call at
  today's size on a quiet host, which puts twenty candidates at about 2 to
  6 seconds of gate time.
- **Budgets.** No change. The step budget and pass deadline bound the worst
  case of every implicit reap; with fast calls they stop binding, and with a
  stale index the fallback still answers several candidates per pass and the
  rest are asked again next pass, which is the gate's existing behavior.
- **A once-per-pass preflight.** niwa runs the `[session.before_remove]`
  command per candidate with no state between runs
  (`internal/cli/removal_gate.go`). It gains an optional `preflight`
  command, run once at the start of a reap pass, before the first ask, and
  reused by every gate the pass builds. A non-zero exit, a timeout or a
  failure to start keeps every candidate in the pass, with the preflight's
  last non-empty line as the reason. Commands that gate outside a pass
  (destroy's preflight, watch's discard) run it once per invocation. The
  per-candidate command and its exit contract don't change.
- **The `--help` probe.** It guards a koto older than 0.27.0, which had no
  verb, and today runs before every real call. dot-niwa moves it into
  `preflight`. Today it falls open (`|| exit 0`), letting a removal go
  ahead under a koto without the verb; in the preflight such a koto exits
  non-zero with a line naming the version it needs, and every candidate is
  kept, the direction D4 asks for. The path-gone and no-koto guards stay
  per candidate, and the preflight exits 0 when no koto is on `PATH` so
  that guard still decides.
- **Batching** stays a rejected alternative (Decision 1, option C).

## Implementation Approach

The phases are the implementation issues, in order. Each lands on its own.
Phase 2 needs phase 1, and phase 5 needs a niwa release carrying phase 4;
the rest are independent of each other.

### Phase 1: The index file, its writers and its rebuild

Add `.path-index.jsonl` with its row format, the shared-lock append at
every writer in the table above (the session-row hook in
`rewrite_header_under_lock`, the witness hook in `replace_log_bytes`), the
move of `write_local_state_bytes` onto `replace_log_bytes`, the rebuild with
its two-step exclusive lock and stat-before-read witnesses, the
housekeeping trigger at `koto next` startup, and
`koto session path-index rebuild`; touches `src/session/local.rs`,
`src/engine/claim.rs`, `src/engine/persistence/append.rs`,
`src/session/links.rs`, `src/session/import_links.rs`,
`src/session/cloud.rs`, `src/cli/mod.rs` and a new module beside
`src/engine/terminal_index.rs`.

### Phase 2: `needed` reads the index

Split `needed()` into one judgment over two candidate sources, add the
staleness rule, the witness and cloud-file stats, the birth-time refusal
and the stderr notice, keep the graph path as a test oracle, and add the
debug check and the index tests above; touches
`src/session/needed.rs`, `src/session/hierarchy/` (to read one session's
facts and lineage without a scan) and the `session needed` section of
`docs/guides/cli-usage.md`.

### Phase 3: `koto session path-index status`

A read-only verb that prints rows, listed sessions, names missing from the
index, sessions whose identity doesn't match their witness, unreadable and
long rows, the last rebuild, and whether `needed` would trust the index, as
text and `--json`; touches `src/cli/mod.rs`, `src/cli/session.rs`, the CLI
guide and the koto-user skill's reference.

### Phase 4: niwa gains a once-per-pass `preflight`

Add the optional `preflight` command to `[session.before_remove]` with the
behavior described in the niwa section; touches
niwa's `internal/config/session.go`, `internal/cli/removal_gate.go` and the
pass code (`internal/cli/pass.go`, `internal/cli/reap_worktrees.go`).

### Phase 5: dot-niwa moves the probe into `preflight`

Once a niwa release with phase 4 ships, move the `koto session needed --help`
guard from the per-candidate command into `preflight`, keep the path-gone
and no-koto guards per candidate, and update the comment; touches dot-niwa's
`.niwa/workspace.toml` only.

### Phase 6: niwa budgets

No change is planned: niwa keeps its 10-second run timeout, 30-second step
budget and 30-second pass deadline. If the acceptance check shows
time-ran-out keeps while `koto session path-index status` reports a trusted
index, that's a niwa issue against `defaultRemovalGateBudget` and
`passDeadline`, filed then with the timings.

### Reading the acceptance checks once it ships

Both run on a host whose store holds at least 9,493 sessions, with the
index trusted (`koto session path-index status`).

1. Run the gate's own call, `koto session needed <path>`, against each
   finished instance, timing each one. Every answer comes back in under two
   seconds, and its exit code and line match what the fallback gives for
   the same path.
2. Run one reap pass over at least twenty candidate instances and read its
   stderr. No `niwa: kept ...` line carries a time-ran-out reason:
   `before_remove timed out`, `time ran out before it could ask`, or
   `the pass's time ran out`.

## Security Considerations

**A wrong index can't remove anything a correct one would keep.** The
reader trusts the index only for absence, and only while the session's
state log has the identity the index last witnessed. A stale or forged row
placing a session under PATH costs a header read. A missing session row is
caught by the listing. A missing continues or import row hides a successor
that would release PATH, so the path stays needed. A koto from before this
change that adopts an anchor or runs `koto session rebind` rewrites the log
by rename, so the identity changes and the fast path reads the real
header. The only in-place overwrites a released koto makes are the cloud
backend's, and any session holding a cloud backend file is read every call.
New sessions an older koto creates are caught by the listing. Without
birth times the fast path is refused.

**Headers damaged outside koto.** Today a header that stops parsing anywhere
in the store turns every answer into 2. koto writes headers by rename, so
its own crashes can't tear one. A tool that rewrites a header by rename
changes the identity, so the fast path reads it, finds it doesn't parse,
and falls back to the exact scan: today's answer. A header damaged in place
(same identity) in a session the fast path doesn't read isn't read, so the
answer comes from the rest of the store. That session's last koto-written
anchor, as the index and its witness record it, is outside PATH, so it
isn't live under PATH, and nothing live is removed.

**Tampering.** The index sits in the sessions directory with the store's
permissions and is opened without following symlinks, as headers are. Anyone
who can rewrite it can already remove the instance or the sessions
themselves, so it adds no new boundary. It records names, ids, anchor paths
and file identities that the store beside it already exposes, and it never
leaves the host.

**Cloud backend.** The bucket walk is unchanged: it runs for the same
running sessions under PATH, with koto#501's limits of 64 pointers, 16
members and 50 requests. The index covers only the local sessions
directory, and the bucket's own name index (`src/session/name_index.rs`) is
separate and untouched.

## Consequences

### Positive

- A `needed` call opens files for the sessions under PATH, their lineage
  and any session changed behind the index, so header reads stop growing
  with every session koto ever ran.
- Twenty candidates fit one reap pass with niwa's budgets unchanged, and
  time-ran-out keeps stop hiding real answers.
- Terminal, cancelled and closed sessions cost a few lstats each per call
  instead of a dozen file operations.
- The continues and import rows give koto a reverse-lineage lookup without
  a scan, which is the root koto#666 names for holder and direct-report
  reads, and which the local continued-by mirror proposed in
  `docs/designs/DESIGN-koto-holder-and-reports-reads.md` could later replace.
- The fallback is about five times cheaper than today's scan, and the
  `--help` probe runs once per pass instead of once per candidate.

### Negative

- Every write path that sets an anchor, writes a continues link or an
  imported-from record must append a row; a new path that forgets slows
  every call down to the fallback.
- Every whole-log replacement appends a witness row, so the index grows
  with re-anchors, claim rewrites and pulls as well as inits, and needs
  compaction.
- The per-session lstats keep the call O(store): estimated 1 to 3 seconds
  quiet at ten times today's store, past the 0.5-second target.
- On a filesystem that reports no birth time the fast path is refused and
  every call costs the fallback.
- Sessions the cloud backend has touched are read on every call.
- `koto session path-index` and niwa's `preflight` are new user-facing
  surface to document and keep.

### Mitigations

- The both-answers debug check fails CI when a tested write path skips its
  hook, and the index tests pin the unaware-writer, in-place, bad-header
  and no-birth-time cases.
- Housekeeping compacts when rows pass twice the session count.
- The stats can go once every koto writer honors a store layout version.
- The stderr line names the failed check and the fix, and
  `koto session path-index status` shows the state and any identity
  mismatches, so a slow gate is never a mystery.
- The verb surface is two subcommands documented with `session needed`;
  `preflight` is one optional field.
