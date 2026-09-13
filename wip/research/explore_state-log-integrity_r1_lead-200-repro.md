# Lead: Can #200 be reproduced, and which mechanism does it point to?

Binary: `target/debug/koto` built from this worktree (`koto 0.12.2-dev+19cd769`).
Isolation: every run exports `KOTO_SESSIONS_BASE=<run>/sessions` and
`HOME=<run>/home` (VERIFIED: `build_local_backend` in src/cli/mod.rs honors
`KOTO_SESSIONS_BASE`; the integration tests set both the same way). Nothing
touched the real `~/.koto`.

Scripts and templates live in `/home/dangazineu/.claude/jobs/d49ea37c/tmp/repro200/`:

| File | Purpose |
|------|---------|
| `common.sh` | env isolation, `check` helper |
| `check.py` | validates line 1 is a header, counts events by type, finds duplicate seqs, fused lines, lost `context_added` appends |
| `inspect.sh` | runs `koto status` / `session list` against a left-behind session |
| `pingpong.md`, `oneshot.md`, `parent.md`, `child.md` | templates |
| `m1_concurrent.sh` | mechanism 1 |
| `m2_rebind_race.sh`, `m2b_adopt_race.sh`, `m2b_analyze.py` | mechanism 2 |
| `m3_rewind_relocate.sh` | mechanism 3 |
| `m4_terminal_cleanup.sh` | mechanism 4 |
| `strace_append.sh` | syscall trace of one append |
| `out-m*.txt`, `runs/` | raw output and the sessions each run left behind |

## Findings

### Mechanism 1: plain concurrent appenders

Command: `m1_concurrent.sh 5 200` -- five rounds, each a fresh session with
three concurrent processes: two looping `koto context add` (200 each) and one
looping `koto next --with-data '{"go":"yes"}'` (200) on a two-state
evidence-driven loop template. 3,000 write attempts in total, plus a smoke run
(`m1_concurrent.sh 1 10`).

Result: header never lost (MEASURED, 0 of 6 rounds). Truncation never seen.
But concurrent appends corrupt the log in two other ways, and every round
ended corrupted:

- **Duplicate seq.** All 5 full rounds wrote a duplicate seq (MEASURED, e.g.
  `m1-r2` seq 345 twice). `append_event` reads the last seq and then appends
  with no lock between the read and the write (VERIFIED,
  src/engine/persistence.rs `append_event` / `read_last_seq`). The reader
  rejects it: `koto status m1-r2` -> `state file corrupted: sequence gap at
  line 347: expected seq 346, got 345` (MEASURED). `context add` keeps
  succeeding after that point, because `read_last_seq` only parses the last
  line, so the log keeps growing while `next` can no longer read it (`next`
  succeeded only 3-35 times out of 200 per round, MEASURED).
- **Fused lines.** Two events landed on one line with no newline between
  them (MEASURED in the smoke run, saved in `runs/smoke-m1`):
  `{"seq":18,...,"type":"transitioned",...}{"seq":18,...,"type":"context_added",...}`.
  Cause: `writeln!` on an unbuffered `File` makes two `write(2)` calls, the
  JSON and then `"\n"` (MEASURED with strace: `write(3, "{\"seq\":4,...", 177)`
  then `write(3, "\n", 1)`). O_APPEND makes each call atomic, not the pair,
  so a second appender can land between them. As the last line, the fused
  line is dropped by the truncated-tail recovery in `read_log_inner`
  (VERIFIED), so the `transitioned` event silently vanishes from replay (the
  session reads as `pong`). Once more lines follow it, it becomes `malformed
  event on line N` corruption (VERIFIED, same function).

### Mechanism 2: whole-file header rewrite racing appenders

`rewrite_header_atomically` (src/engine/claim.rs) reads the file, writes a
temp file of header + old tail, and renames it over the log. It takes no lock
(VERIFIED). CLI triggers: `koto session rebind` (src/cli/session.rs
`handle_rebind`) and anchor adoption in `koto next` when the header has no
`execution_dir` (src/cli/mod.rs, the `ExecutionAnchorCheck::Adopt` arm).

**2a, rebind.** `m2_rebind_race.sh 5 300`: two `context add` loops (300
each) against one `session rebind` loop alternating `--to a` / `--to b` (300).
4,500 writes. Result (MEASURED):

| round | ctx ok | ctx in log | lost | dup seqs | header |
|-------|-------:|-----------:|-----:|---------:|--------|
| 1 | 600 | 298 | 302 | 3 | ok |
| 2 | 600 | 302 | 298 | 4 | ok |
| 3 | 556 | 277 | 279 | 3 | ok, 1 fused line |
| 4 | 600 | 301 | 299 | 6 | ok |
| 5 | 12 | 6 | 6 | 1 | ok, fused last line |

About half of the successfully reported appends are silently lost. The
rename puts back a snapshot taken before they landed. The header survived in
every round. In round 5 a fused last line made `read_last_seq` fail for every
later writer: `failed to parse last event line: trailing characters` (228
times from rebind, MEASURED), and only 12 of 600 context adds succeeded. That
is a third failure mode: a torn tail blocks all further appends.

**2b, anchor adoption.** `m2b_adopt_race.sh 200 6`: 200 fresh sessions,
each with `execution_dir` stripped from the header to simulate a
pre-anchoring session. A `context add` loop (6 adds) runs concurrently with
one `koto next` that adopts. Result: 191 of 200 rounds lost at least one
append, 193 lost in total, header intact in all 200 (MEASURED).
`m2b_analyze.py` shows 146 of the lost keys were the one written between the
`execution_anchor_adopted` append and the rename, which is the window the
code predicts (MEASURED plus VERIFIED).

`rewrite_header_identity` (src/session/local.rs) is the other rewrite. It
uses plain `fs::write` (O_TRUNC, then write). Its only CLI path is
`relocate`, reached from parent rewind (mechanism 3), and `session recover`.
By construction it can only lose appends or expose an empty or partial file
to a concurrent reader. It can't leave a lone event line, since it always
writes the header plus every line it read (VERIFIED). An appender that reads
the file mid-truncate sees no events, and `read_last_seq` returns 0, so it
writes `seq:1`: duplicate-seq corruption, not header loss (INFERRED).

Conclusion: rewrites lose history silently and tear seqs. None of them can
produce #200's single-line, headerless shape: every rewrite writes the header
it just parsed as line 1 (VERIFIED).

### Mechanism 3: parent rewind relocates a child while a writer uses the old name

`relocate` renames the directory `<parent>.<task>` to `<parent>~N.<task>`,
renames the state file, and rewrites the header identity (VERIFIED,
src/session/local.rs `relocate`; called from `rewind_relocate_children`,
src/cli/mod.rs). Nothing is left at the old name. `context add` never checks
that the session exists: `LocalBackend::add` does
`fs::create_dir_all(<base>/<session>/ctx)`, which recreates the session
directory, and then `append_event` opens the log with
`create(true).append(true)` (VERIFIED, src/session/local.rs `add` and
src/engine/persistence.rs `append_event`).

**Part A, deterministic** (`m3_rewind_relocate.sh`, first half): parent
`orch` materializes `orch.task-a` and `orch.task-b`, then `context add
orch.task-a pre`, then `koto rewind orch` (`children_relocated: 2`), then
`context add orch.task-a post`. The last command exits 0 and leaves
(MEASURED):

```
{"seq":1,"timestamp":"2026-09-13T03:36:38.562Z","type":"context_added","payload":{"key":"post","hash":"5891b5...","size":6}}
```

That's one line, no header, the latest write. Then `koto status orch.task-a`
returns `state file corrupted: failed to parse header: missing field
`workflow` at line 1 column 179`, `koto next orch.task-a` returns the same
error as a `persistence_error`, and `koto session list` returns `['orch',
'orch~1.task-a', 'orch~1.task-b']`, so the session is missing from the list.
`list()` silently skips unparseable headers (VERIFIED, src/session/local.rs
`list`). The history isn't destroyed. It sits intact under `orch~1.task-a`,
but nothing points the writer there.

**Part B, race** (`m3_rewind_relocate.sh 50 20`): 50 parents, each with a
20-iteration `context add` loop on `<p>.task-a` racing `koto rewind <p>`.
All 50 old-name logs ended headerless (MEASURED). With several late writes
they hold several lines starting `seq:1, seq:1, seq:2 ...`, the exact shape
#236 shows. 6 of 1,000 context adds that reported success appear in neither
the old-name log nor the relocated copy (MEASURED). Those are appends lost
across the rename-and-rewrite window.

### Mechanism 4: terminal cleanup, then a late or concurrent write

A session reaching a terminal state runs `backend.cleanup(name)`, which is
`remove_dir_all` of the session directory (VERIFIED, src/cli/mod.rs terminal
path near `append_child_completed_to_parent`; src/session/local.rs
`cleanup`).

**Part A, deterministic** (`m4_terminal_cleanup.sh`, first half): `init
t1`, three `context add`s (header plus 6 events), then `next t1 --with-data
'{"go":"yes"}'` reaches `done`, and the sessions directory is empty. Then one
`context add t1 late` exits 0 and leaves exactly one line (MEASURED):

```
{"seq":1,"timestamp":"2026-09-13T03:36:40.634Z","type":"context_added","payload":{"key":"late","hash":"5891b5...","size":6}}
```

`koto session list` returns `[]`, and `koto status t1` returns `failed to
parse header: missing field `workflow` at line 1 column 179`. Here the
history really is gone. The directory was deleted, so no other copy exists.

**Part B, race** (`m4_terminal_cleanup.sh 100 10`): 100 sessions, each a
10-iteration `context add` loop racing the terminal `next`. 81 of 100 ended
with a headerless log (MEASURED). The other 19 are a second defect: the
concurrent `context add` refilled `ctx/` while `remove_dir_all` ran, cleanup
failed with `session cleanup failed: Directory not empty (os error 39)` (38
rounds), and the terminal session was left on disk with its header.
`fully_cleaned=0`, so no round ended clean.

## Implications

#200's three observations match mechanisms 3 and 4 and not the others:

1. **A lone `context_added` line, the latest write.** Only an append to a
   log that doesn't exist writes one event as the whole file. Every rewrite
   path writes the header it just read back as line 1 (VERIFIED). No
   concurrent-append or rewrite race lost the header in ~7,500 writes plus
   200 adoption races (MEASURED).
2. **Header and ~45 events gone.** Under mechanism 4 the old log was deleted
   by terminal cleanup. Under mechanism 3 it was moved to `<parent>~N.<task>`.
   The report reads the loss as truncation, but it's consistent with the file
   having been deleted or renamed and a new one created (INFERRED).
3. **Missing from `koto session list`.** `list()` silently skips any session
   whose header doesn't parse (VERIFIED). Both mechanisms reproduce it
   (MEASURED).

Mechanism 4 fits the story in #200 best (INFERRED). A subagent drove the
session on its own: it submitted `scrutiny_outcome`, advanced into `review`,
and submitted `review_outcome`. If that pushed the work-on session to a
terminal state, cleanup deleted the directory. When the parent agent then
wrote context "partway through the next phase", `context add` recreated the
session with a single headerless line. Mechanism 3 needs a parent rewind
across a `materialize_children` state, and nothing in #200 mentions one.

So `Fixes #200` is honest if the fix makes `context add` (and every
`append_event` caller) refuse to create a missing log, the same fix #236
needs. #200 and #236 are the same bug reached two ways. The report's own
guess, a truncate-and-rewrite racing an appender, doesn't produce this shape,
though that race is real and loses data in other ways (below).

Separately from #200, concurrent writers corrupt logs today, and a PR that
claims to fix "concurrent writers" has to cover these:

- Lost appends from `rewrite_header_atomically` racing appenders: about 50%
  under rebind load, and 191 of 200 adoption races.
- Duplicate seqs from the unlocked read-then-append in `append_event`. These
  make the log unreadable (`sequence gap`).
- Fused lines from the two-syscall `writeln!`. A fused line mid-file is
  corruption. As the last line it silently drops an event from replay and
  makes every later append fail.
- Cleanup failing with `ENOTEMPTY` when a write races `remove_dir_all`,
  which leaves a terminal session behind.

## Surprises

- The two-syscall `writeln!` (JSON, then newline) was not on anyone's list.
  It means O_APPEND gives no line atomicity at all. The fix is one `write_all`
  of a buffer that already ends in the newline. A lock over append would also
  cover it.
- Duplicate seqs show up in every round of plain concurrent appending. A
  concurrent `context add` next to a `koto next` (the exact #200 setup) is
  enough to make a session unreadable with `sequence gap`, and no rewrite is
  needed. It's a different error string from #200's, but just as permanent.
- Relocation doesn't destroy history. The data sits under `<parent>~N.*`.
  Terminal cleanup does destroy it.
- `count_unreadable()` skips directories containing `~`, but relocated copies
  are healthy, so this didn't matter here. The headerless old-name logs are
  counted as unreadable (VERIFIED, src/session/local.rs).
- Adoption rewrites are frequent enough to matter in practice. Every session
  created before anchoring existed adopts on its first `next`, and that tick
  races whatever else is writing.

## Open Questions

- Did the #200 session actually reach a terminal state before the bad write?
  The report says it was "auto-cleaned later", which a headerless log can't
  reach through `next`. Either a later `session cleanup` or workspace prune
  removed it, or the reporter conflated the two events. The shirabe work-on
  template's transitions after `review_outcome` would settle whether the
  subagent's evidence could have made it terminal.
- The cloud backend wraps the local one (`CloudBackend::cleanup` calls
  `self.local.cleanup`). I didn't test whether a sync can resurrect a
  directory the same way. Not measured.
- For the 47 m2b lost keys outside the predicted window: those rounds had
  several adds in the window, so they may be the same cause counted per key.
  I didn't break them down further.
- Should `append_event` also refuse when the existing file's first line
  isn't a header (#200's "integrity guard")? It would stop a stale writer
  from extending a headerless log (#236's multi-line shape) but doesn't
  prevent creating one.

## Summary

#200's exact shape (one headerless `context_added` line, history gone,
missing from `koto session list`) reproduced deterministically from the CLI
by a `context add` to a session whose directory is gone. That happens after
terminal cleanup deletes it (81 of 100 races too) or after a parent rewind
relocates it (50 of 50 races), because `context add` recreates the directory
and `append_event` creates the log headerless, the same root cause as #236
(MEASURED and VERIFIED). None of the ~7,700 concurrent-append and
header-rewrite racing writes (m1, m2a and the m2b adoption races) lost a
header: every rewrite path writes the header back as line 1, so the
truncate-and-rewrite theory in #200 doesn't explain it, and `Fixes #200` is
honest for a "refuse to append to a missing log" fix. Those races are still
destructive in their own right, though: `rewrite_header_atomically` silently
drops about half of concurrent appends, unlocked read-then-append writes
duplicate seqs that make the log unreadable, and `writeln!`'s two `write(2)`
calls fuse lines. So the lock-and-atomic-write level of the fix has
independent MEASURED evidence.
