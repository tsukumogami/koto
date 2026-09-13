# Scrutiny: intent review, koto#236 (+ #200)

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'` (commits fddc4aa, d52dd0f).
Intent: no koto write path can produce a state log whose first line isn't a
header, and a refused context write leaves nothing behind.

Verdict: 0 blocking, 6 advisory. The implementation matches the intent for
every sequential path. What remains is concurrency, which is the deliberately
deferred write-lock work. The PR body should word the cloud-pull claim so it
doesn't overstate what the guard closes (A1).

Test run: I ran the suites relevant to this change with
`CARGO_TARGET_DIR` isolated, and all passed: `--lib` 1572,
`integration_test` 200, `claim_sidecar` 21, `respawn` 15,
`state_log_integrity_test` 8. An earlier full-suite run was cut off by a
pipe, so I'm not claiming the whole workspace suite. About 30 suites it did
reach were green.

## How every append was checked

Every production event append in `src/` goes through
`persistence::append_event_in::<H>` or `append_event_idempotent_in::<H>`, and
both now call `next_seq_after_header::<H>` (src/engine/persistence.rs:525),
which refuses a missing file, an empty file, an empty first line, or a first
line that doesn't parse as `H`, before anything is opened for writing. The
append opens with `.append(true)` and no `create` (persistence.rs:172-175,
385-388), so a log removed between the check and the open fails the open.
`acquire_state_flock` no longer creates the file (persistence.rs:411-414).

The callers I traced:

- Backend funnel: `LocalBackend::append_event` (src/session/local.rs:189-196)
  calls `persistence::append_event`. `CloudBackend::append_event`
  (src/session/cloud.rs:729-738) delegates to local first, so the cloud backend
  is covered too. All `backend.append_event` callers in cli/mod.rs (2212, 2456,
  2585, 3021, 3624, 3870, 4006, 4365, 4453, 4679, 5256, 5309, 5461, 5724, 6408),
  cli/retry.rs (410, 428, 556), cli/overrides.rs:286, cli/session.rs:577 and
  cli/context.rs (50, 156) are covered.
- Raw-path appends that bypass the backend:
  - `session update` at src/cli/session.rs:494 checks `backend.exists`
    (session.rs:486), then calls `persistence::append_event` directly. The
    guard covers it.
  - The wake pass at src/engine/wake.rs:336 only runs when the coordinator log
    exists (src/cli/mod.rs:3435). The guard covers the race anyway.
  - src/engine/respawn.rs:548 and :569, and src/engine/claim.rs:626 and :677,
    all go through `persistence::append_event`. None of them has a production
    caller today; only tests/respawn.rs and tests/claim_sidecar.rs call them,
    and both tests write a header first.
- Request-store logs: src/engine/request_store/mod.rs:1256 now uses
  `append_event_idempotent_in::<RequestHeader>`, so request logs are checked
  against their own header type rather than skipping line 1 unseen. There are
  only two `LogHeader` impls (persistence.rs:50, request_store/mod.rs:444), and
  both are wired correctly.
- `append_header` / `append_header_line` (persistence.rs:80-116) can create a
  file, but only with a header line, and its only non-test caller is the
  backend trait delegation. Nothing in production calls `backend.append_header`.
- Non-append writers of state logs are `init_state_file` (temp plus rename,
  header plus events), `rewrite_header_atomically` (src/engine/claim.rs:370-420,
  temp plus rename, parses the header first), `rewrite_header_identity`
  (src/session/local.rs:659-685, parses the header first, but the rewrite
  itself is truncate-then-write through `fs::write`), and the cloud pulls
  (cloud.rs:155, 546, 627, which are `fs::write` of remote bytes). None of them
  appends, and none creates a headerless log from a well-formed one except by
  crashing or failing partway through a truncating write. In that case the
  guard now refuses every later append, which is what the binding review asked
  for.

A path that previously did produce a headerless log and is now closed, though
the PR doesn't mention it: `ChildCompleted` appended to a parent that has
gone away (src/cli/mod.rs:2585). Before this change, `create(true)` made a
headerless parent log. Now the append is refused, the caller logs a warning,
and child cleanup is deferred (`ChildCompletedAppend::AppendFailed`). That's
the right outcome.

The CLI contract matches the approved scope. `LocalBackend::exists` is "state
file exists" (local.rs:81-83), so the exit-2 flat envelope at
src/cli/mod.rs:1422-1434 and :1484-1494 covers never-initialized names,
cleaned-up sessions, rewound children, blocked children, and a directory with
no log. The pre-check sits before `store.add`, so no session directory gets
created. For a log that exists but has no header, `backend.read_header` at
src/cli/context.rs:26 and :149 refuses before `store.add` / `store.remove`, and
the error reaches the caller as `EXIT_INFRASTRUCTURE` = 3 (cli/mod.rs:75),
which is what docs/reference/error-codes.md documents.

## Advisory findings

### A1. Check-then-append still leaves a narrow concurrent window for a truncating writer (advisory: deferred write-lock scope)

`next_seq_after_header` reads and validates the log, and then a separate
`OpenOptions::append` writes it (persistence.rs:160-175). Suppose a concurrent
truncating writer runs between those two steps: the cloud `sync_pull_state`
(cloud.rs:155, `fs::write`, on every `read_events`/`read_header`) or
`rewrite_header_identity` (local.rs:682). If it truncates, the O_APPEND write
lands at offset 0, and if the truncating writer then fails or crashes before
writing its content, the result is a log whose only line is our event. That's
#200's shape again.

This is an append producing a headerless log, but only through a concurrent
truncating writer, and the approved scope puts the write lock, atomic rewrites
and the cloud-pull fix in the follow-up. So I'm rating it advisory, not
blocking. The guard does close the sequential versions: a crash mid-rewrite,
or a failed pull that left an empty file, is refused by every later append.
Two changes to make:

- PR body and CHANGELOG: say the header guard closes the cloud-pull and
  crash-mid-rewrite cases *when they happen before the append*. A pull that
  truncates *during* an append is left to the write-lock follow-up. The binding
  review's wording, "closed by the header guard", is too strong for the
  concurrent case.
- wip/followup_state-log-write-lock.md: add this window explicitly. The lock
  has to cover the header check and the write as one critical section, and the
  cloud pull's local write has to go through temp plus rename under that same
  lock.

### A2. "A refused context write leaves nothing behind" holds sequentially but not under a race (advisory)

The context path is three separate steps: the `backend.exists` check
(cli/mod.rs:1426), then `backend.read_header` (context.rs:26), then
`store.add` (context.rs:41), then the append (context.rs:50). Terminal cleanup
or a rewind relocate can land between `read_header` and the append. When it
does, `store.add` has already run `fs::create_dir_all` on the session's `ctx/`
(src/session/local.rs:497), which recreates the session directory and writes
the content, and then the append is refused. What's left is a directory with
`ctx/` and no log: not headerless, but exactly the stale context a later spawn
would inherit, which is what #236's tests guard against sequentially. This is
the same ENOTEMPTY/cleanup race family the follow-up already lists. Worth one
line in the follow-up note.

### A3. Foundation for the write-lock follow-up is sound, with one structural constraint to record (advisory)

Nothing here makes the lock harder. Several things make it easier:

- The header type is now a generic parameter on the append entry points
  (`append_event_in::<H>`, `append_event_idempotent_in::<H>`), so one locked
  append primitive can serve both session and request logs.
- No append or append-lock path creates a file any more. That matches the
  follow-up's rule that the lock never creates the directory.
- The guard and the seq read sit in one function, `next_seq_after_header`,
  which is the natural body for the critical section.

The constraint to record: the refusal the context verbs promise lives above
the backend (context.rs:26 and :149, and cli/mod.rs:1426 and :1488), while the
append sits inside `backend.append_event`. A lock taken only inside
`LocalBackend::append_event` won't cover `store.add`, so A2 stays open. The lock
has to be taken at the context-verb level and span check, store and append.
`flock` locks taken on two separate open file descriptions in the same process
conflict with each other, so the inner append can't naively re-take the same
sidecar lock. Either pass a held guard down, or give the backend a
`with_write_lock(id, |..|)` shape. The follow-up design should decide this
explicitly.

### A4. The claim.rs `.audit.jsonl` fallback is now a guaranteed error, and its doc comment is stale (advisory)

`coord_state_file_for` (src/engine/claim.rs:592-610) looks for
`<sessions>/<coord>/<coord>.state.jsonl`, which isn't the real naming. Real
logs are `koto-<coord>.state.jsonl` (src/session/mod.rs:171-173). When that
file doesn't exist, the function falls back to
`<session_dir>/<coord>.audit.jsonl`. That file is never created with a header,
so `append_redelegated_audit` (claim.rs:612-630) now always fails there, and
`recover_orphaned_sidecar` returns `Err`. Before this PR the fallback created a
headerless JSONL file, so the guard correctly killed a headerless-producing
path.

It has no production caller today; tests/claim_sidecar.rs:70-72 writes a
headered `<coord>.state.jsonl`. But the comment at claim.rs:585-591 ("we fall
back to writing the audit event into the child's directory") is now false, and
once this gets wired up the naming mismatch will make recovery fail every time.
I recommend fixing the comment, or deleting the fallback, in this PR or as a
tiny follow-up, and flagging the `<coord>.state.jsonl` versus
`koto-<coord>.state.jsonl` mismatch to the coordinator as a candidate issue.
Relatedly, `claim_and_dispatch` and `append_redelegated_audit` both call
`create_dir_all` on the coordinator's directory before the append
(claim.rs:621-624 and :672-675). If the append is refused, that leaves an empty
coordinator directory behind. It's harmless, but it's the same
"refusal leaves something behind" smell.

### A5. Cloud `context add` on a session that exists only in S3 exits 3, not 2 (advisory)

`CloudBackend::exists` falls back to an S3 check (cloud.rs, the `exists` impl),
so a session with no local directory passes the exit-2 pre-check. Then
`read_header`, through `sync_pull_state`, can't write the pulled log because
nothing creates the directory (cloud.rs:150-156 has no `create_dir_all`). It
only warns, and `local.read_header` fails, so the verb exits 3 with a
file-not-found message. Nothing gets written, so the intent holds, but the
error isn't the documented `workflow '<name>' not found`. This predates the
PR's scope, because cloud pull semantics are deferred. I'm noting it so the
follow-up's cloud-pull rework also defines what `context add` should do here.

### A6. The idempotent hash scan still skips line 1 unseen (advisory, harmless)

`append_event_idempotent_in` (persistence.rs:336-372) scans
`content.lines().skip(1)` for a prior hash *before* the header check. On a
headerless log whose first line happened to carry the same hash, it would
return `Idempotent` without error. Nothing gets written, so this can't produce
or extend a headerless log, and request logs are already guarded by
`read_view_at` under `request.lock` (request_store/mod.rs:1235). It's only a
consistency nit: moving `next_seq_after_header` ahead of the scan would make
"refuses a log with no header" true of every return path, not just the writing
ones.

## Contract spot-checks (all match)

- The exit-2 flat envelope `workflow '<name>' not found` with the right
  `command` value: cli/mod.rs:1426-1434, :1488-1494.
- No `ctx/` and no session directory after a refusal: the pre-check sits
  before `store.add`, and tests/state_log_integrity_test.rs:103-141 asserts it.
- The blocked-batch test goes on past the refusal: A is completed, the parent
  ticks, B spawns, B is readable, and B has no leftover context
  (tests/state_log_integrity_test.rs:375-450).
- The rewind test exists (state_log_integrity_test.rs:495).
- `read_last_seq` no longer skips line 1 blindly. It was replaced by
  `next_seq_after_header` (persistence.rs:525-567).
- The error says "state log has no header" (persistence.rs:627-641), and the
  manual fix is documented in docs/reference/error-codes.md.
- The CHANGELOG calls out the exit-2 behaviour change and doesn't claim
  terminal cleanup caused #200.
