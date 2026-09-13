# Lead: What does koto's state-file lock actually protect, and what should its semantics be?

## Findings

### There are two "state-file locks", and #171 names the wrong one

**`acquire_state_flock`** (`src/engine/persistence.rs:399-428`) is the one #171 cites. It
opens the log with `create(true).read(true).write(true)` and calls blocking
`flock(LOCK_EX)` (`persistence.rs:419`). Its only caller is `append_event_idempotent`
(`persistence.rs:314`), and that function has exactly one production caller: the request
store (`src/engine/request_store/mod.rs:1256`). That caller already holds `request.lock`, a
sidecar acquired non-blocking with sleep-and-retry against a deadline
(`request_store/mod.rs:1021-1063`, taken at `:1229`). So in production the blocking flock
is nested inside a sidecar lock, never contends, and never touches a session state log.
The only thing that depends on it blocking is `tests/idempotency.rs:318`
(`n32_concurrent_identical_retries_collapse_to_one_write`). That test calls
`append_event_idempotent` from 32 threads with no outer lock, and any error other than
`ConcurrentSubmissionConflict` panics (`idempotency.rs:~350`). Switching this flock to
`LOCK_NB` would break that test and nothing else.

**`lock_state_file`** (`src/session/local.rs:343-379`) is the lock `koto next` actually
takes. It opens the log **read-only, without create** (`local.rs:353-356`), so a missing log
returns `NotFound`. It then calls `flock(LOCK_EX | LOCK_NB)` (`local.rs:364`) and maps
`EWOULDBLOCK` to `SessionError::Locked { holder_pid: None }` (`local.rs:367-373`). The trait
doc says it's non-blocking by contract (`src/session/mod.rs:362-406`). The cloud backend
delegates to the local one (`src/session/cloud.rs:843-853`), so it serializes same-host
callers only. Its single production caller is `handle_next`, and only when
`state_is_batch_scoped` is true (`src/cli/mod.rs:4375-4396`). That check is simply "the
current state has a `materialize_children` hook" (`cli/mod.rs:2349-2351`). On contention it
emits `BatchError::ConcurrentTick` with exit 1 (`cli/mod.rs:4379-4383`;
`src/cli/batch_error.rs:47-55, 451`; `docs/reference/error-codes.md:309`).

So the lock the design is talking about already matches Decision 3's "non-blocking with
immediate error". The real doc-vs-code gaps are elsewhere:

- **Coverage.** `DESIGN-auto-advancement-engine.md` Decision 2 (lines 124-139) put a flock
  around every advancement loop, so a second `koto next` couldn't "interleave writes and
  produce duplicate sequence numbers". Decision 3 (lines 141-157) made it non-blocking. The
  stated reason was that blocking waits are "harder to reason about in signal-heavy code"
  and hide latency from the agent. The code narrowed the lock to batch parents only. The
  comment at `cli/mod.rs:4358-4365` justifies that with "the engine's existing
  single-writer semantics", and `tests/integration_test.rs:1750-1808` pins it: a
  non-batch `koto next` must ignore an external flock.
- **Dead error code.** `concurrent_access` is documented (`error-codes.md:64`) and defined
  (`src/cli/next_types.rs:722, 763`), but nothing emits it.
- **Lock target.** The batch design specified a sidecar, `<session_dir>/<workflow>.lock`
  (`DESIGN-batch-child-spawning.md:2259-2268, 3694-3695`). The code locks the log itself.

### How long it's held, and what it doesn't cover

For a batch parent, the guard lives for the rest of `handle_next`. That includes
`advance_until_stop` (`cli/mod.rs:4693`), which runs gate commands and actions. Each command
defaults to a 30-second timeout (`src/action.rs:14, 156-160`), polling gates can run for
their declared `polling.timeout_secs` (`src/template/types.rs:394-398`), and the loop chains
through several states. So a tick can hold the lock for minutes.

Even inside `handle_next`, some writes land before the lock is taken: the
`evidence_submitted` append (step 5, `cli/mod.rs:4341`) and the execution-anchor adoption,
which appends and then does a header rewrite (`cli/mod.rs:3600-3625`).

None of these writers take any lock on the log:

- `context add` and `context remove` (`src/cli/context.rs:45, 147`)
- `rewind` and its child relocation (`cli/mod.rs:2188, 2295`)
- `rebind`, an append followed by `rewrite_header_atomically` (`src/cli/session.rs:577-584`)
- intent updates (`session.rs:494`, which calls `persistence::append_event` directly)
- overrides (`src/cli/overrides.rs:286`)
- retry (`src/cli/retry.rs:410, 428, 556`)
- child-completed notifications (`cli/mod.rs:2432, 2561`)
- claim header rewrites (`src/engine/claim.rs:432, 569`)
- `relocate`, which calls `rewrite_header_identity` (`src/session/local.rs:306-340, 659-685`)
- recover (`src/session/recover.rs:234`)
- the cloud backend's `sync_pull_state`, which `fs::write`s the remote bytes over the local
  log on every `read_events` and `read_header` (`cloud.rs:150-164, 740-754`)

### Other locks in the codebase (none cover the state log)

- **Context store:** a per-key `<key>.lock` and `ctx/manifest.lock`, both blocking
  `LOCK_EX` (`local.rs:455-489, 507-537`). They protect ctx content and the manifest.
  `strace` of `koto context add` shows these two flocks and **no** flock on the log: the log
  is opened plainly with `O_APPEND`.
- **`claim.lock`:** an `O_EXCL` exactly-one-winner lease (`claim.rs:1-64, 233-286`). It's a
  claim marker with stale-recovery, not a mutex around log writes.
- **Epoch fencing** (`tests/epoch_fencing.rs`): a pre-write validation of
  `--dispatch-epoch` against the header. It's not a lock.
- **Terminal index:** a compaction lease, `_terminal_index.compact.lock`
  (`src/engine/terminal_index.rs:453-626`).
- **`request.lock`:** the request store's sidecar, described above.
- There's no session-directory lock.

### Rename-replace and truncate against a lock held on the log

flock binds to the open file description, and so to the inode.

`rewrite_header_atomically` reads the log, writes a temp file, and renames it over the log
(`claim.rs:370-420`). The rename gives the path a new inode, which has three consequences:

- A current `lock_state_file` holder keeps its lock on the orphaned old inode.
- The next locker opens the new inode and gets its lock immediately, so two "exclusive"
  holders run at once.
- Any append that lands between the rewrite's `read_to_string` and its `rename` is lost.
  The header survives, but the history loses a line, and a missing seq is a sequence gap
  the reader refuses.

`rewrite_header_identity` (`local.rs:682`) and the cloud pull (`cloud.rs:155`) use
`fs::write`, which keeps the same inode but opens with `O_TRUNC` and writes from offset 0.
The lock survives, but other things break:

- A reader in the window sees an empty or partial file.
- An `O_APPEND` append that lands after the truncate gets overwritten.
- If the rewriter dies between the truncate and the write, the file is left empty, or holds
  only whatever a concurrent appender wrote. That's a one-line, headerless log, the shape
  #200 reports. I haven't reproduced this; the #200 repro lead owns it.

Locking the log itself can't protect any of this unless every rewrite happens in place.
Rename-replace is the durable way to rewrite, so the lock belongs on something the rename
doesn't replace.

Separately, a lock can't help when the log is missing. `context add` runs
`store.add` → `create_dir_all(ctx_dir)` (`local.rs:497`) *before* `append_event`. So a
writer holding a stale path after `relocate` or cleanup recreates the directory, and
`append_event`'s `create(true)` (`persistence.rs:159-170`) creates a headerless log.
`lock_state_file`'s open-without-create, which returns `NotFound`, is the right behavior
there. Any new lock must likewise never create the session directory or the log.

### Pure appenders are not safe: seq assignment is read-modify-write, and each event is two write() calls

`append_event` reads the last seq and then opens with `O_APPEND` to write `seq+1`
(`persistence.rs:145-181`), with no lock between the two steps. `strace -y` of one
`koto context add` shows the event going out as **two** syscalls: `write(fd, "{...}", 177)`
followed by `write(fd, "\n", 1)`. That's because `writeln!` goes to an unbuffered `File`.
The request store's own torn-tail comment says the same thing. On local Linux filesystems,
one `write()` to an `O_APPEND` regular file sets its offset and writes its bytes atomically
with respect to other writers (PIPE_BUF applies to pipes, not regular files), but two
writes per event let two appenders produce `A B \n \n`. NFS gives no such guarantee.
`terminal_index.rs:14-24` cites PIPE_BUF as its rationale, which is the wrong rule. It
still works in practice because it issues a single `write_all` (`terminal_index.rs:221`).

**Reproduced.** I ran 120 concurrent `koto context add` calls (three rounds of 40) against
one freshly initialized session, using `target/debug/koto` built from 19cd769, in three
separate runs. Two of the three runs ended with duplicate seqs (2 and 6 of them). After
those runs, `koto status` fails with `state file corrupted: sequence gap at line 40:
expected seq 39, got 38`, which bricks the session with no rewriter involved. No torn lines
appeared in these runs. The reproduction scripts are `/tmp/kotolock_race.sh` and
`/tmp/kotolock_strace.sh`. The request store's docs spell out this exact failure as the
reason every request write takes `request.lock`: "two unlocked concurrent appends computing
the same next sequence would make the request permanently unreadable"
(`docs/workspace-layout.md:80-89`, `DESIGN-request-lifecycle.md:1079-1095`). The session
log never adopted the same rule.

## Implications

**The lock is needed for appender-vs-appender, not just rewriter-vs-appender.** Emitting the
line in one `write()` would stop torn lines but not duplicate seqs. The read-last-seq,
compute, append sequence needs mutual exclusion across every writer of the session log.

**Recommended coverage rule.** Every write to a session state log, append or rewrite, takes
one exclusive **write lock** for the duration of its read-modify-write. Put it inside
`persistence::append_event` (or `LocalBackend::append_event` plus the direct
`persistence::append_event` call in `session.rs:494`), `rewrite_header_atomically`,
`rewrite_header_identity` (also converted to temp+rename), `relocate`, and the cloud
backend's pull-write. Enforcing it at the persistence layer means no CLI verb can forget
it; it's the same shape as `request.lock` being taken inside `append_with`.

**Lock target: a sidecar in the session directory, not the log.** For example
`<session_dir>/state.lock`, opened with `O_CREAT|O_NOFOLLOW` but only inside a directory
that already exists (no `create_dir_all`). With a sidecar:

- Rename-replace of the log no longer orphans the lock.
- Acquiring the lock never extends or truncates the log.
- `relocate` moves the lock along with the directory.
- A writer holding a stale path fails to open the sidecar because the directory is gone,
  and gets a clean refusal instead of recreating the session.

This matches both the batch design's own `<workflow>.lock` and the `request.lock`
precedent. `flock` on the session-directory fd would also work on Linux and macOS, with no
extra file, but it departs from existing practice. Once a writer holds the lock, it checks
that the log exists and has a header before appending, and refuses otherwise. That's the
#236 guard, and it only works under the lock.

**Two locks with different semantics, so the long tick doesn't starve short writers.**

- **Tick lock (the existing one, moved to a sidecar).** Held across a batch parent's whole
  `koto next`, including gates. Keep it non-blocking, fail fast, with `concurrent_tick`,
  exit 1, as both designs specify and the code already does. `rewind` changes the state a
  tick is reasoning about, so it should probably take the same tick lock, also fail-fast.
- **Write lock (new).** Held only for milliseconds around each append or rewrite. Every
  writer takes it, including `koto next`'s own `append_closure`, per append, and not across
  gates. Use **bounded wait**: non-blocking with retry up to a short deadline, then a
  transient, retryable error. The request store's `acquire_request_lock` already works
  this way, with 20 ms retries and a 5 s default deadline (`request_store/mod.rs:108`), and
  could be factored out and shared.

**Why neither pure blocking nor pure fail-fast works for the write lock.** If `context add`
took a single lock that `koto next` holds across gates, a blocking `context add` would hang
for up to minutes: 30 s per gate command, polling timeouts, chained states. A fail-fast
`context add` would error spuriously exactly when an agent and its subagent write at the
same moment, which is #200's scenario. A short critical section plus bounded wait gives
correct seqs with no visible failures in practice, and a wedged holder (SIGSTOP, a hung
filesystem) becomes a clear error instead of a hang. Flock releases when the holder dies,
which is why the request-lifecycle design picked flock over an exclusive-create lease.

**#171's answer, as far as coverage needs it.** The lock `koto next` takes is already
non-blocking, as designed. The blocking `acquire_state_flock` is a redundant inner lock on
the request path. Delete it, or leave it and re-document it; don't flip it to `LOCK_NB`,
which would only break `tests/idempotency.rs:318`. The N=32 retry guarantee should rest on
the outer lock. What #171 should record is the real gap: the design locked every
advancement loop, the code locks only batch parents, and it never locked any other writer.
The write lock above closes the corruption half of that gap. Whether non-batch `koto next`
also needs the tick lock is a separate logical question (two ticks both transitioning).
It's out of scope here, and `integration_test.rs:1750-1808` pins the current behavior.

**Cloud.** flock is host-local, and `CloudBackend` delegates to it already. The unlocked
`fs::write` in `sync_pull_state` is a rewriter that runs on every read under the cloud
backend. It needs the write lock and temp+rename like any other rewriter. Cross-host races
stay out of scope, as they already are for the request store.

## Surprises

- #171 attributes the blocking `LOCK_EX` to the `koto next` path. In fact it's the
  request-store-only `append_event_idempotent`, and the `koto next` lock is `LOCK_NB`.
- Concurrent `koto context add` alone, with no rewrite, no `next`, and no rewind, bricked a
  session in two of three runs by producing duplicate seqs. That's a reproduced,
  permanently fatal corruption in the same concurrent-writers family as #200, though with a
  different error message ("sequence gap", not "missing field workflow").
- Each event goes out as two `write()` calls, payload then newline, so even the `O_APPEND`
  atomicity argument doesn't hold for koto's own appends.
- The batch design specified a sidecar lock file; the code locks the log itself, and
  rename-based rewrites defeat that.
- `concurrent_access` is documented and typed but never emitted.
- Even for batch parents, the evidence append and the anchor-adoption header rewrite in
  `handle_next` happen *before* the lock is taken.
- `acquire_state_flock` opens with `create(true)`, which is another create-if-missing path.
  It only reaches request logs today.

## Open Questions

- Should writers open the log through the locked directory fd (`openat`), so that a
  `relocate` finishing while they wait sends their write to the relocated session, or makes
  it refuse, rather than landing on a stale path? And which of those is correct for a child
  renamed by `rewind`?
- Should `rewind` take the parent's tick lock (fail-fast)? Should `relocate` take each
  child's write lock before renaming it?
- What deadline should the write lock use, 5 s like the request store or shorter, and what
  error code should it surface (reuse `concurrent_access`, or add a new code)?
- If `acquire_state_flock` is removed, should the N=32 idempotency test be re-pointed at
  the new write lock?
- Would making `append_event` emit the line and its newline in one `write()` still be worth
  doing as defense in depth, since the reader only tolerates a torn *final* line?

## Summary

koto has two state-file locks. #171 cites the wrong one: the blocking `LOCK_EX` in
`persistence.rs` guards only request-store appends (under an existing sidecar lock), while
`koto next` locks the log non-blocking (`LOCK_NB`, `concurrent_tick`) and only for batch
parents, so every other writer (`context add`, rebind, rewind, relocate, header rewrites,
the cloud pull) runs with no lock on the log at all. That gap is directly harmful. Seq
assignment is read-then-append and each event takes two `write()` calls, so 120 concurrent
`koto context add`s produced duplicate seqs that permanently bricked the session in two of
three runs, and rename-based header rewrites would orphan any lock held on the log's inode
anyway. Recommendation: every writer takes a short write lock on a sidecar in the session
directory (bounded wait, then a transient error, as the request store does), refusing to
write if the log is missing. The long batch tick lock stays non-blocking and fail-fast, as
the design specifies, and #171 is re-scoped to coverage rather than blocking semantics.
