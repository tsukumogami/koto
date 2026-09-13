# Exploration Findings: state-log-integrity

## Core Question

Which fix levels (refuse appends to a missing log; lock coverage plus atomic
rewrites; recovery of headerless logs already on disk) belong in one PR for
#236 and #200, and what evidence backs each?

## Round 1

### Key Insights

- #236's mechanism holds exactly (lead-236-mechanism, lead-write-paths).
  `koto context add` has no existence check (src/cli/mod.rs:1417-1433),
  `ContextStore::add` creates the session directory (src/session/local.rs:497),
  `persistence::append_event` opens with `create(true).append(true)`
  (src/engine/persistence.rs:159-160), and `read_last_seq` skips line 1 blindly
  (persistence.rs:518), which yields the duplicate `seq:1`. `context remove` has
  the same bug when the directory exists without a log. The new
  `tests/state_log_integrity_test.rs` fails 6 of 7 on main, including the full
  blocked-batch-child shape and the terminal-cleanup shape.
- #200's exact shape reproduces from the CLI (lead-200-repro): a `context add`
  to a session whose directory was deleted by terminal cleanup (deterministic;
  81/100 in a race) or moved by a parent rewind (deterministic; 50/50 in a race)
  leaves one headerless `context_added` line, and `session list` hides the
  session. No concurrent-append or header-rewrite race lost a header in about
  7,700 racing writes plus 200 adoption races. #200 and #236 are one bug
  reached two ways, so a fix that refuses to create a missing log honestly
  fixes #200. Terminal cleanup fits #200's story best (a subagent drove the
  session forward; the parent later added context).
- Concurrent writers corrupt logs in three other, measured ways
  (lead-200-repro, lead-lock-semantics): duplicate seqs from the unlocked
  read-last-seq-then-append (every round of plain concurrent appending; the log
  then fails with `sequence gap` permanently); lost appends from
  `rewrite_header_atomically`'s unlocked read-temp-rename (about half under
  rebind load, 191 of 200 adoption races); and fused lines, because `writeln!`
  on an unbuffered `File` issues two `write(2)` calls.
- The lock covers almost nothing (lead-write-paths, lead-lock-semantics). The
  only production lock is a non-blocking flock on the log inode at
  src/cli/mod.rs:4377, taken only when `koto next` starts on a
  `materialize_children` state, and after the adoption, `--to`, `retry_failed`
  and evidence writes. Four whole-file rewriters take no lock:
  `rewrite_header_identity` (truncate+write; relocate, recover),
  `rewrite_header_atomically` (temp+rename; rebind, anchor adoption), and the
  cloud backend's `sync_pull_state` (overwrites the local log on every read)
  and resolve pulls. A rename-replace orphans any lock held on the log's inode.
- #171 cites the wrong lock (lead-lock-semantics). The blocking `LOCK_EX`
  (persistence.rs:419) is inside `append_event_idempotent`, used only by the
  request store under its own sidecar lock. The `koto next` lock is already
  `LOCK_NB` with `concurrent_tick`, as the design specifies. The real gap is
  coverage.
- Recovery (lead-recovery): a tolerant reader can't make a #236 log tickable
  (no init events, unrecoverable template hash) and would hide #200-style
  history loss. The pain in #236 is the stranded parent: the scheduler hides
  the headerless child from `list()`, treats it as unspawned, and its atomic
  no-replace init collides on every tick (src/cli/batch.rs:1485-1590,
  src/session/local.rs:294). The scheduler's existing half-init repair pass
  (batch.rs:647-677) is the natural place to move a headerless child log aside.
- Agent surface (lead-agent-surface): a refusal needs no new error code;
  `context add` uses the flat envelope, so `workflow '<name>' not found` at
  exit 2 matches `status` and `next`, and it makes the long-standing promise
  in docs/guides/cli-usage.md:452 true. 13 tests in tests/integration_test.rs
  (3073-3603) write context to a bare directory and must init first; one
  asserts on the headerless log. Docs to touch: koto-user
  command-reference.md, docs/reference/error-codes.md, batch-workflows.md,
  CHANGELOG `[Unreleased]`.

### Tensions

- Lock semantics: fail-fast suits the long batch tick, but a `context add`
  that fails fast would error exactly in #200's agent-plus-subagent scenario,
  and a blocking one could hang behind a tick that holds the lock across gates.
  Resolved by two locks: the existing tick lock stays non-blocking, and a new
  short write lock with bounded wait is taken per write.
- Recovery vs masking: rename-aside of a headerless batch child can't tell a
  never-started child (#236) from a live child whose history was lost (#200).
  Keeping the moved file and warning keeps it from being silent.
- Scope size: full lock coverage (including the cloud pull) is larger than the
  #236 fix; the brief requires it, but the cloud pull's cross-host behavior is
  a different class of problem.

### Gaps

- The `respawn.rs` and claim appends/rewrites are dead code today (no
  production callers); not traced further.
- Cloud backend behavior with a headerless log already pushed to S3 is not
  measured.
- The `ENOTEMPTY` cleanup race (a `context add` refilling `ctx/` during
  `remove_dir_all`) was measured but no fix direction chosen.

### Decisions

See wip/explore_state-log-integrity_decisions.md (round 1).

### User Focus

Taken from the task brief: weigh the three fix levels before any code; lock
coverage decided and written down with #171's question settled; a deliberate
#200 reproduction recorded; a decision on recovering existing logs.

## Accumulated Understanding

There is one root cause behind both headerless-log reports: every state-log
append goes through `persistence::append_event`, which creates a missing file,
and `context add` recreates a missing session directory before appending. Any
session whose log is absent (never started, cleaned up at terminal, relocated
by rewind) becomes a permanently unreadable one-line log on the next
`context add`. Refusing to create a missing log, at the verb (exit 2, before
`store.add`) and in `append_event` itself, closes it for every writer, and
fixes both #236 and #200.

Separately, and with its own measured evidence, concurrent writers corrupt
logs today without losing the header: duplicate seqs, lost appends under
rename-replace, and fused lines. Fixing that means every writer takes a short
write lock on a sidecar file in the session directory (bounded wait, like the
request store's `request.lock`), every rewrite goes through temp+rename under
that lock, and each event goes out in one `write`. The existing tick lock stays
non-blocking and fail-fast; #171 becomes a coverage question, answered by the
write lock.

Recovery is limited: nothing in a headerless log can rebuild a session, so a
tolerant reader is rejected. A clearer "log has no header" error plus a
scheduler repair that moves a headerless batch child's log aside (keeping
`ctx/`) heals stranded batches from 0.12.x.

Open: whether the cloud backend's pull-overwrite goes in this PR, and the
exact write-lock deadline and error surface.

Review (coordinator, round 1): truncation is ruled out for local rewrites and
appends only. The cloud pull's `fs::write` and a crash mid-rewrite can leave an
empty or headerless file, so the guard must be "the first line parses as a
header", not "the file exists". The terminal-cleanup story for #200 stays
INFERRED: in shirabe's work-on template only `blocking_escalate` reaches a
terminal state.

## Decision: Crystallize
