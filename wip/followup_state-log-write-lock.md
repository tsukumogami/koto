# Follow-up: state-log write lock and atomic rewrites

Deferred from the #236/#200 fix by the coordinator on 2026-09-12. This work
enters at `/scope`. It references #200 and #171 but doesn't close them. The
full evidence is in the round-1 research files (lead-write-paths,
lead-lock-semantics, lead-200-repro). This note carries the conclusions so they
survive wip/ cleanup; copy it somewhere durable before the branch merges.

## Measured corruption from concurrent writers (main at 5b8cfbf)

- **Duplicate seqs.** `persistence::append_event` reads the last seq and then
  appends, with no lock in between. Mixed `context add` plus `next` wrote
  duplicate seqs in 5 of 5 rounds, and 120 concurrent `context add`s alone did
  it in 2 of 3 runs. The log then fails permanently with `sequence gap`.
- **Lost appends.** `rewrite_header_atomically` (engine/claim.rs) reads the log,
  writes a temp file and renames it over the log, with no lock. It lost about
  half of the concurrent appends under `session rebind` load, and 191 of 200
  anchor-adoption races lost at least one append.
- **Fused lines.** `writeln!` on an unbuffered `File` issues two `write(2)`
  calls (the JSON, then the newline), confirmed with strace. O_APPEND makes
  each call atomic, but not the pair. A fused last line makes `read_last_seq`
  fail for every later writer.
- **ENOTEMPTY on cleanup.** A `context add` refilling `ctx/` during
  `remove_dir_all` fails terminal cleanup (38 of 100 rounds), leaving the
  session on disk.

## What writes a state log today, and what locks it

- The only production lock is `lock_state_file` at cli/mod.rs:4377:
  `LOCK_EX|LOCK_NB` on the log inode, and only when `koto next` starts on a
  `materialize_children` state. It's taken after the adoption, `--to`,
  `retry_failed` and evidence writes.
- These write with no lock:
  - `context add` and `context remove`
  - rebind
  - rewind/relocate (`rewrite_header_identity`, which truncates and writes)
  - a child's `ChildCompleted` append to its parent (cli/mod.rs:2561)
  - `session update` (a raw `persistence::append_event`, bypassing the backend)
  - the wake pass (a raw path, bypassing the backend)
  - the cloud `sync_pull_state` (`fs::write` over the local log on every read)
  - the cloud resolve pulls
- A rename-replace orphans any lock held on the log's inode.

## #171

The blocking `LOCK_EX` it cites (persistence.rs:419, `acquire_state_flock`) only
guards request-store appends, which already sit under `request.lock`. The
`koto next` lock is already non-blocking fail-fast (`concurrent_tick`), as
DESIGN-auto-advancement-engine Decision 3 specifies. The real gap is coverage:
the design locked every advancement loop, and the code locks only batch parents
and no other writer. Leave #171 open; closing it needs the user.

## Proposed direction (needs a design)

- Every state-log writer takes a short write lock on a sidecar file in the
  session directory, never creating the directory. Use bounded wait and then a
  retryable error, reusing the request store's `acquire_request_lock` pattern
  (20 ms retries, 5 s deadline).
- Every whole-file rewrite goes through temp and rename under that lock,
  including `rewrite_header_identity` and the cloud pull's local write.
- Each event goes out in a single `write_all`.
- The batch tick lock stays non-blocking fail-fast.
- Decisions still open: the sidecar path, the deadline, the contention error
  shape and exit code for flat-envelope verbs (`concurrent_access` is documented
  but never emitted), whether `rewind` takes the tick lock, and whether writers
  should open through the locked directory fd.

## What the #236 fix leaves open (from its scrutiny review)

- The header check and the append are two steps. If a truncating writer (the
  cloud `sync_pull_state` `fs::write`, or `rewrite_header_identity`) truncates
  between them and then fails, the append can still leave a lone headerless
  line, which is #200's shape. The write lock has to span the check and the
  append.
- `context add` reads the header, then `store.add`, then appends. If cleanup or
  a rewind lands between those steps, `ctx/` content is left in a directory
  with no log. The lock should be taken at the context-verb level so it covers
  the header check, the store write and the append.
- flock locks on two separate file handles conflict even within one process.
  So a lock held by the verb has to be passed down to the backend's append,
  not taken again inside it.
- The `.audit.jsonl` fallback in `engine/claim.rs` (`coord_state_file_for`)
  looks for `<coord>.state.jsonl` rather than `koto-<coord>.state.jsonl`, so it
  always falls back. The header check now refuses that append. There's no
  production caller yet; fix both before wiring the recovery up.

## Three things the lock design has to settle (from the code review)

- The context verbs check the header twice: once in the verb, once inside the
  append. Under a lock held across both, one check is enough.
- `acquire_state_flock` (the request store's inner lock on the log inode) and a
  new sidecar lock would overlap. Decide whether the inner one goes.
- The request store already takes `request.lock` and then `acquire_state_flock`,
  so it locks twice per write today.

## Acceptance tests the design should require

- Race rebind and anchor adoption against `context add`, not only plain
  appenders, for about 20 rounds. Assert zero lost appends and no duplicate
  seqs; m2b fails 191 of 200 on main.
- A unit test that one event goes out in a single write.

## Other items to propose as issues (not filed)

- The `ENOTEMPTY` cleanup race above.
- "Not found" exits 1 on `rewind`, `cancel` and `decisions record`, but 2 on
  `status` and `next`.
- The scheduler moving a headerless batch child's log aside, keeping `ctx/`,
  so stranded 0.12.x batches heal. This needs the user's call.
- `session update` and the wake pass append through raw paths that bypass the
  backend, so there's no cloud push.
