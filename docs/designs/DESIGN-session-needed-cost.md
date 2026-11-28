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
  and id, plus the continues links and imported copies the lineage check
  follows. Every code path that writes a header's anchor, a continues link or
  an imported-from record appends its row first, so the index only ever
  over-reports. `needed` reads the index, lists the store's names once, and
  opens files only for sessions the index places under PATH and their
  lineage neighbours, judging each one from its own header and log exactly
  as today. When the index is missing, damaged, or doesn't name every
  session the listing shows, `needed` answers from an exact header-only scan
  instead, with one stderr line saying why and naming
  `koto session path-index rebuild`. Answers, exit codes and lines don't
  change, and niwa's per-path contract and budgets stay as they are.
rationale: |
  Only an index makes the per-call cost independent of the store: a
  narrower scan cuts today's work by about five times but is still a pass
  over every session, about ten seconds at ten times today's store, and
  batching in niwa changes niwa's engine-neutral contract and still costs
  one full scan per pass. Treating rows as candidates and re-reading the
  truth for every session the answer relies on means a stale row can only
  cost time; the only thing trusted from the index is absence, and the
  listing check catches the common way absence goes wrong. Falling back to
  the exact scan rather than answering 2 keeps removals working while the
  index heals, and niwa's own timeout still keeps an instance if the
  fallback runs long.
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

Every `needed` answer in under 2 seconds at today's store size or larger,
and twenty candidate instances answered in one reap pass with no
time-ran-out keep. The verb's answers don't change: same exit codes, same
line, same meaning. This design changes only how fast the verb gets there.
The two ways a store can be changed behind koto's writers (an older koto
rebinding a session, a header damaged by something other than koto) are
named in Security Considerations with what bounds them.

## Decision Drivers

- **D1. Same answers.** Exit 0, 1 and 2 and the printed line stay exactly
  as `needed()` computes them today, including its edges: a session with an
  unreadable header anywhere in the store makes the answer 2 unless another
  session needs PATH, and a PATH that can't be canonicalized is 2.
- **D2. Cost bounded by the path, not the store.** Under 2 seconds per
  call is the requirement; the design target is under 0.5 seconds at about
  9,600 sessions and at ten times that, so twenty candidates fit one
  30-second step with room left for the pass's other work.
- **D3. niwa's contract stays.** One engine-neutral command per path, exit
  status only. Nothing in niwa should need to know koto has an index.
- **D4. Fail toward keeping.** Anything the index gets wrong may cost
  time or keep an instance; it must never let a running session's
  directory be removed.
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

#### Chosen: a path index kept by koto's writers (B)

koto keeps an append-only file in the sessions directory with one row per
session (name, header id, kind, execution_dir), one per continues link
(holder, named predecessor, recorded id) and one per imported-from record
(copy, source name, source id). Every code path that writes one of those
facts appends its row first. `needed` reads the index, lists the store's
names once to confirm the index knows every session, and then opens files
only for the sessions the index places under PATH and for the lineage
neighbours of the running ones. Each of those is judged from its own header,
links and log, through the same per-session code as today, so the index
supplies candidates and the files supply the answer.

At today's size the index is roughly 1.5 to 2 MB and the listing is 9,600
names; at ten times the store, about 15 to 20 MB and 96,000 names. Both are
sequential reads with no per-session open. The estimate is tens of
milliseconds today and a few hundred at ten times the store, plus a handful
of file opens per session under PATH. These are estimates from the row size
and today's per-open cost, not timings; the acceptance check below times
them.

#### Alternatives considered

**A narrower scan (A).** Drop the graph build: one header read per session
to find the candidates and spot unreadable headers, then read lineage only
when a running session is under PATH, by listing each session's
`koto/links/continues/` and `koto/imported-from`. A finished instance with
no running session needs only the header pass. This is what
`session list --execution-dir` already costs: 4.8e9 instructions and about
1.07 seconds today on a quiet host, two to four times that under load, and
about ten seconds at ten times the store. It needs no new file and can't go
stale. Rejected as the main path because it stays O(store) and fails D2 at
scale; twenty candidates at 1 second already fill two-thirds of a step
today. It's kept as the fallback (Decision 2), where it beats today's scan
by about five times with the same answer.

**Batching in niwa (C).** niwa would collect a pass's candidates and ask
once, and koto would answer every path from one scan. One scan per pass
instead of one per candidate fixes the step budget at today's size. But it
changes niwa's gate from one engine-neutral command per path into a
multi-path protocol every engine would have to speak (D3), each pass still
pays a full scan (1.65 to 6.55 seconds today, 16 to 65 at ten times the
store, past the 30-second deadline), and a single call no longer meets a
2-second bound. A batch contract would need its own niwa design; it isn't
needed once a single call is fast.

### Decision 2: What `needed` answers when it can't trust the index

The index can be missing (a store from before this change, or a deleted
file), damaged (a line that doesn't parse), or incomplete (a session
directory the index has no row for, which means some writer skipped it). The
verb has to answer something, and the answer must never move toward removal
(D4).

#### Chosen: answer from the exact narrow scan, and say so on stderr

`needed` runs option A's header-only scan, which gives the same answer as
today at about a fifth of the cost, and prints one line to stderr before its
answer, for example:

```text
koto: the path index doesn't list 3 sessions in this store, so this answer came from a full scan; run `koto session path-index rebuild` to fix it
```

The line names which check failed and the command that fixes it (D5). It's
printed before the answer because niwa takes the last non-empty line as the
keep reason, so the answer line stays the reason exactly as today. If the
fallback runs past niwa's 10-second timeout at a much larger store, niwa
kills it and keeps the instance, which is the safe direction. `needed` never
writes the index itself (D6); it's rebuilt by the verb the line names or by
the next koto housekeeping pass (Solution Architecture).

#### Alternatives considered

**Answer 2 while the index is stale.** Cheap and safe, but every removal in
the workspace stops until someone rebuilds, which turns a missing file into
an outage of the reaper. An exact answer at about 1 second today is better
than no answer. Rejected.

**Fall back to today's `Graph::scan` path.** Same answer, five times the
work of option A. Rejected; the graph path stays only as the test oracle.

### Decision 3: What changes on niwa's side

#### Chosen: nothing in niwa; dot-niwa drops its per-candidate probe

At 0.5 seconds a call, twenty candidates take about ten seconds of gate
time, inside both the 30-second step budget and the 30-second pass deadline
with room for git reads and removals. niwa's budgets stay as they are. The
`koto session needed --help` probe in dot-niwa's declaration goes; see the
niwa section of Solution Architecture for what that changes.

#### Alternatives considered

**Raise niwa's step budget or pass deadline.** That would make today's slow
calls fit, but every implicit reap at create, dispatch and watch can already
spend about 90 seconds in gate runs (mapped reap, backstop, worktree sweep),
and a longer budget makes that worst case longer for every user while fixing
nothing in koto. Rejected; it can be revisited if the acceptance check shows
time-ran-out keeps with a healthy index.

**A version check once per pass instead of the probe.** niwa runs the
declaration as an independent shell command per candidate, with no state
between runs, so a once-per-pass check would need new niwa surface for one
workspace's guard. Rejected.

## Decision Outcome

koto gains a path index, `.path-index.jsonl` in the sessions directory, kept
by the code that writes the facts it records. Each row is one short JSON
line appended the way `_terminal_index.jsonl` appends
(`src/engine/terminal_index.rs`): `O_APPEND`, at most 4 KiB, fsync. A
session row records name, header id, kind and execution_dir; a continues row
records holder, named predecessor and recorded id; an import row records
copy, source name and source id. Writers append the row before the write it
describes, so a crash between the two leaves a row with no matching write,
never a write with no row. Rows are never edited in place; a rebuild rewrites
the file from a full scan under a lock that writers hold shared.

`needed` stays one call per path. It canonicalizes PATH as today, reads the
index, and lists the store's directory names once. If the index is missing,
damaged, or lacks a row for any listed name, it prints one stderr line and
answers from the exact header-only scan. Otherwise it collects candidates:
every ordinary session with any recorded anchor under PATH, every ordinary
session the index records with no anchor (a first tick can still adopt one),
and every session the index flags as unreadable or too long to record. It
reads each candidate's real header and keeps today's per-session judgment:
kind, anchor, `under_dir`, `inspect`, `held()`, the bucket walk under the
cloud backend, the sort by name, the count, the 200-byte cut. `held()` gets
its lineage from the index's continues and import rows, verified by reading
those holders' and copies' own link files, headers and logs. Sessions the
index places elsewhere are never opened.

A read-only `koto session path-index status` reports what the index knows
and whether `needed` would trust it, and `koto session path-index rebuild`
rebuilds it. koto's existing housekeeping at the start of `koto next` (where
the terminal index is compacted) also rebuilds an index that's stale,
oversized or a day old, best effort, under the same lease discipline.

On niwa's side nothing changes in code. dot-niwa drops the per-candidate
`--help` probe. Pruning old terminal sessions isn't part of this design and
isn't needed for it.

### Rationale

The index changes what the verb reads, not what it decides. Every fact the
answer rests on still comes from the session's own files; the index only
says which sessions to open, and the only thing taken on its word is that a
session not listed under PATH isn't anchored there. Row-before-write
ordering makes the index a superset of what writers did, the listing check
catches writers that never wrote a row, and a full fallback that announces
itself covers everything else. The trade-off accepted is a second record of
each session's anchor that writers must keep, and a residual case (a koto
from before this change re-anchoring an existing session into PATH) that the
listing can't see; Security Considerations covers it.

## Solution Architecture

### Overview

```text
writers (init, adoption, rebind, rename, import, pull, link --continues)
   |  1. append row to <sessions>/.path-index.jsonl   (shared lock)
   |  2. write the header / link / imported-from       (as today)
   v
koto session needed PATH
   canonicalize PATH -> read index -> list store names
        | trusted                          | missing/damaged/incomplete
        v                                  v
   candidates from rows              stderr notice, exact header-only scan
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
```

A `built` row opens every rebuilt file. `unreadable` comes only from a
rebuild that found a header it couldn't parse. A row that wouldn't fit in
4 KiB (an anchor near the platform's path limit) is written as `long`
instead, which tells the reader to open that session's header on every call.
Removals write nothing: a removed session simply stops appearing in the
listing, and its rows are ignored until the next rebuild drops them. The
reader folds rows into sets, not latest-wins: a session with three recorded
anchors is a candidate for all three until a rebuild, which keeps the
ordering of concurrent appends from mattering.

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

Every header rewrite goes through `claim::rewrite_header_under_lock`
(`src/engine/claim.rs`), which already holds the new header before its
rename. The hook goes there: when the new header's `execution_dir`, `kind`,
id or name differs from the old one, append a session row, then rename.
Header rewrites that touch only claims or epochs append nothing, so the row
rate follows inits and re-anchors, not ticks.

### Rebuild and compaction

A rebuild runs option A's scan plus a read of every session's continues
links and imported-from record, writes a new file, and renames it over the
old one. It runs from `koto session path-index rebuild`, and from the
housekeeping at `koto next` startup when the index is missing, damaged,
lacks a row for a listed session, has more than twice as many rows as the
store has sessions, or was last built more than a day ago. Housekeeping
checks at most once every ten minutes, using the lock file's mtime as its
clock, so a tick pays one stat in the common case. The daily rebuild is what
bounds the one case the listing can't see (Security Considerations).

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
4. Candidates: ordinary sessions with any recorded anchor under PATH
   (`under_dir`, whole components), ordinary sessions recorded with no
   anchor, and every `unreadable` and `long` row. Each candidate then gets
   the state-file check `session_names` makes, and its real header is read.
5. Judge each candidate exactly as `needed()` does today. For a running one,
   `held()` looks up continues rows whose `peer` is the session's name or
   whose `peer_id` is its header id, and import rows whose `from` is its
   name, then reads those holders' and copies' own facts with
   `SessionFacts::read` and applies the same resolution `build_directed`
   and `find_migrated` apply. The session rows' ids give the id-to-name map
   that resolution needs.
6. Under the cloud backend, the bucket walk runs for the same sessions as
   today, with the same limits.
7. Print and exit as today.

The implementation keeps one judgment function and gives it two candidate
sources, the index and the narrow scan. Tests run both, and today's graph
path as an oracle, over the same fixture stores and require identical
answers. A debug-only environment switch makes `needed` compute both answers
and abort when they differ, and the integration suite runs with it on, so a
new write path that forgets its hook fails CI instead of shipping.

### Staleness, in one rule

The index is trusted only when it parses, its format is known, and every
session directory the store listing shows has a row; otherwise `needed`
answers from the exact header-only scan and says so on stderr. Inside that
rule, an index that over-reports costs a few extra header reads, and one
that's out of date for a session it does list can't hide that session from
PATH as long as every writer appended before writing.

### Cost

| | Today (about 9,600 sessions) | Ten times |
|---|---|---|
| Today's `needed` (graph scan plus header pass) | about 22e9 instructions, 1.65 to 6.55 s | about 220e9, 16 to 65 s |
| Option A, the fallback (header pass) | about 4.8e9, about 1.07 s quiet, 2 to 4 s loaded | about 48e9, about 10 s quiet |
| Option B, the index (estimated) | tens of ms plus a few opens per session under PATH | a few hundred ms |

The fast path is O(sessions under PATH) in file opens; the index parse and
the name listing grow with the store but are sequential reads with no
per-session open. A path with many sessions under it (a workspace root
rather than an instance) costs what those sessions cost, as it does today.

### Old terminal sessions

The scan doesn't look at status: a session cancelled months ago costs the
same as a running one, and so does a closed anchor. Cancelling abandoned
runs doesn't make the current verb faster; they would still be read on every
call. Under the index they cost one row and one listed name and are never
opened unless they're under PATH. Pruning finished sessions from the store
would shrink the fallback's scan and the index, but store pruning is on hold
for now, and this design neither waits for it nor changes what prune keeps.

### The niwa side

niwa's gate and its contract stay as they are: one command per path, exit
status only, 10 seconds per run, 30 seconds per step, and on main the
30-second pass deadline.

- **Per-call target.** Twenty candidates in one 30-second step need well
  under 1.5 seconds a call, and less once the pass deadline's git reads and
  removals are counted. The design's target is under 0.5 seconds per call
  at today's size and at ten times it, which puts twenty candidates at about
  ten seconds of gate time.
- **Budgets.** No change. The step budget and pass deadline bound the worst
  case of every implicit reap; with fast calls they stop binding, and with a
  stale index the fallback still answers several candidates per pass and the
  rest are asked again next pass, which is the gate's existing behavior.
- **The `--help` probe.** dot-niwa's declaration runs
  `koto session needed --help` before every real call, so each ask starts
  two koto processes. It guards a koto older than 0.27.0, which had no
  verb; the declaration already says it needs 0.27.0. Dropping it removes a
  process start per candidate. A koto without the verb then exits 2 with
  clap's usage error and the instance is kept with that line as the reason,
  instead of removed: a move toward keeping, and the reason says what to
  upgrade. The other two guards (path gone, no koto on `PATH`) stay.
- **Batching** stays a rejected alternative (Decision 1, option C).

## Implementation Approach

The phases are the implementation issues, in order. Each lands on its own;
phase 2 needs phase 1, and the rest are independent of each other.

### Phase 1: The index file, its writers and its rebuild

Add `.path-index.jsonl` with its row format, the shared-lock append at
every writer in the table above (with the `rewrite_header_under_lock` hook),
the rebuild with its two-step exclusive lock, the housekeeping trigger at
`koto next` startup, and `koto session path-index rebuild`; touches
`src/session/local.rs`, `src/engine/claim.rs`, `src/session/links.rs`,
`src/session/import_links.rs`, `src/session/cloud.rs`, `src/cli/mod.rs` and
a new module beside `src/engine/terminal_index.rs`.

### Phase 2: `needed` reads the index

Split `needed()` into one judgment over two candidate sources (index and
narrow scan), add the staleness rule and the one-line stderr notice printed
before the answer, keep the graph path as a test oracle, and add the
both-answers debug check to the integration suite; touches
`src/session/needed.rs`, `src/session/hierarchy/` (to read one session's
facts and lineage without a scan) and the `session needed` section of
`docs/guides/cli-usage.md`.

### Phase 3: `koto session path-index status`

A read-only verb that prints rows, listed sessions, names missing from the
index, unreadable and long rows, the last rebuild, and whether `needed` would
trust the index, as text and `--json`; touches `src/cli/mod.rs`,
`src/cli/session.rs`, the CLI guide and the koto-user skill's reference.

### Phase 4: dot-niwa drops the `--help` probe

Remove the `koto session needed --help` guard from the
`[session.before_remove]` command and update its comment; touches dot-niwa's
`.niwa/workspace.toml` only, and can land any time after phase 2 ships.

### Phase 5: niwa budgets

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

**A wrong index can't remove anything a correct one would keep, except in
one case.** The reader trusts the index only for absence. A stale or forged
row that claims a session is under PATH costs a header read. A missing row
for a whole session is caught by the listing. A missing continues or import
row means a successor that would release PATH isn't found, so the path stays
needed. The one way absence goes wrong is a session that's listed but whose
row records an old anchor, and that only happens when the re-anchor skipped
the hook: a koto from before this change, run against the same store after
the index exists, adopting an unanchored session (covered, since unanchored
sessions are always re-read) or running `koto session rebind` into PATH (not
covered). New sessions that koto creates are caught by the listing, which
makes a mixed-version store fall back early and visibly. The release notes
say to rebuild the index after downgrading or when an older koto shares the
store, and the daily housekeeping rebuild bounds how long a missed rebind
can stand to a day.

**Headers damaged outside koto.** Today a header that stops parsing anywhere
in the store turns every answer into 2. koto writes headers by rename, so
its own crashes can't tear one; a header damaged afterwards by something
else, in a session the index places elsewhere, isn't read by the fast path
and doesn't turn the answer into 2 until a rebuild records it as
`unreadable`. That's a move from "can't decide" to the answer the rest of
the store gives, for a session that by its last recorded anchor isn't under
PATH.

**Tampering.** The index sits in the sessions directory with the store's
permissions and is opened without following symlinks, as headers are. Anyone
who can rewrite it can already remove the instance or the sessions
themselves, so it adds no new boundary. It records names, ids and anchor
paths that the headers beside it already hold, and it never leaves the host.

**Cloud backend.** The bucket walk is unchanged: it runs for the same
running sessions under PATH, with koto#501's limits of 64 pointers, 16
members and 50 requests. The index covers only the local sessions
directory, and the bucket's own name index (`src/session/name_index.rs`) is
separate and untouched.

## Consequences

### Positive

- A `needed` call opens files for the sessions under PATH and their lineage,
  not for the store, so the gate's cost stops growing with every session
  koto ever ran.
- Twenty candidates fit one reap pass with niwa's budgets unchanged, and
  time-ran-out keeps stop hiding real answers.
- Terminal, cancelled and closed sessions cost one row each per call instead
  of a dozen file operations.
- The continues and import rows give koto a reverse-lineage lookup without
  a scan, which is the root koto#666 names for holder and direct-report
  reads, and which the local continued-by mirror proposed in
  `docs/designs/DESIGN-koto-holder-and-reports-reads.md` could later replace.
- The fallback is about five times cheaper than today's scan.

### Negative

- Every write path that sets an anchor, writes a continues link or an
  imported-from record must append a row; a new path that forgets slows
  every call down to the fallback, or in the rebind case can hide a session.
- A koto from before this change sharing the store after the index exists
  can rebind a session into PATH without a row.
- The index grows by a row per init and re-anchor and needs compaction.
- A stale index makes every call as slow as the fallback until a rebuild,
  and a rebuild costs one full scan.
- `koto session path-index` is new user-facing surface to document and keep.

### Mitigations

- The both-answers debug check in the integration suite fails CI when any
  tested write path skips its hook.
- The listing check turns any session an older koto creates into a visible
  fallback, and the daily housekeeping rebuild bounds how long a missed
  rebind can stand; release notes cover mixed versions.
- Housekeeping compacts when rows pass twice the session count.
- The stderr line names the failed check and `koto session path-index
  rebuild`, and `koto session path-index status` shows the state, so a slow
  gate is never a mystery.
- The verb surface is two subcommands, documented with the `session needed`
  section they serve.
