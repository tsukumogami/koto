# Lead: every write path to a session state log

Research at worktree HEAD 19cd769 (main 5b8cfbf plus the scope commit). Read-only.
Every citation is `file:line` under `src/`.

A session's state log is `<base>/<id>/koto-<id>.state.jsonl` (`session::state_file_name`),
with `<base>` = `~/.koto/sessions` (session/local.rs:39-44).

## Inventory

"Lock" means the state-file flock. Only two functions take one:
`LocalBackend::lock_state_file` (session/local.rs:343-379) and
`persistence::acquire_state_flock` (engine/persistence.rs:399-428).

### Primitives

| Path (function, file:line) | Operation | Creates if missing | Lock | Reached from |
|---|---|---|---|---|
| `persistence::append_event`, engine/persistence.rs:139-182 (open at :159-160) | append, `create(true).append(true)`; seq taken from `read_last_seq`, which skips line 1 unconditionally (:513-529, skip at :518) | yes, the file only (the parent dir must already exist); a missing file gets seq 1 (:145-149) | no | every backend `append_event`, plus the direct callers below |
| `persistence::append_header` / `append_header_line`, engine/persistence.rs:80-116 | `create(true).write(true).truncate(false)` (:91): **writes at offset 0 without O_APPEND**, so on an existing file it overwrites the first bytes | yes | no | no production caller. `LocalBackend::append_header` (session/local.rs:184-187) and `CloudBackend::append_header` (session/cloud.rs:719-727) exist, but only tests call them |
| `persistence::append_event_idempotent`, engine/persistence.rs:292-390 | append under `acquire_state_flock` (blocking `LOCK_EX` on the log file itself, opened `create(true)`, :404-419) | yes, via the lock open | yes, blocking | only production caller is the request store (engine/request_store/mod.rs:1256) on `requests/<id>/request.jsonl` (request_store/mod.rs:82-84). **Never a session state log** |
| `LocalBackend::init_state_file`, session/local.rs:215-304 | temp file `.koto-init-*.tmp` in the session dir (:269-273), fsync, chmod 0600, then create-exclusive rename (`atomic_create_rename` :294 → engine/atomic_fs.rs:98-134) | yes: creates the session dir (:231) and the log; fails with `Collision` if a log (even a headerless one) is already there | no (exclusive create makes one unnecessary) | `koto init` (cli/mod.rs:1911 → cli/init_child.rs:481-628); `koto init --from-stdin` (cli/mod.rs:2059 → init_child.rs:785-786); `koto session start` (cli/session.rs:373-374); batch child spawn inside `koto next` on a parent (cli/batch.rs:1569, 1655); retry respawn (cli/retry.rs:613, 660) |
| `LocalBackend::relocate`, session/local.rs:306-340 | **rename** the session dir (:326), **rename** the log (`rename_state_file` :637-647, `fs::rename` :640), then `rewrite_header_identity` (next row) | no; fails if the source is missing (:314-318) | no | `koto rewind` → `rewind_relocate_children` (cli/mod.rs:2209, 2295), only when the from-state has a `materialize_children` hook (:2256-2264). Every `<parent>.<task>` child is moved to `<parent>~N.<task>` |
| `rewrite_header_identity`, session/local.rs:659-686 | read whole file, replace line 1, **`fs::write` = truncate + write in place** (:682) | effectively yes (`fs::write` creates), but it reads first and fails if the log is missing | no | `relocate` above (local.rs:337); `koto session recover --apply` → `recover_one` (session/recover.rs:234, after a dir `fs::rename` :211 and `rename_state_file` :220; cli/session.rs:703; cli/mod.rs:1390-1391) |
| `rewrite_header_atomically`, engine/claim.rs:370-421 | read (:374), write temp `.<logname>.tmp.<pid>` in the same dir (:388-416), then **`fs::rename` over the log** (:418): new inode | no (reads first) | no | `koto session rebind` (cli/session.rs:581, right after an append at :577); `koto next` execution-anchor adoption (cli/mod.rs:3613, right after an append at :3600). Also claim.rs:432 and :569, which are unreachable (see below) |
| `LocalBackend::cleanup`, session/local.rs:85-92 | **delete**: `remove_dir_all` on the session dir | n/a | no | terminal auto-clean `finish_terminal_tick` (cli/mod.rs:2724), reached from `koto next` natural path (:5449) and `--to` path (:3994); `koto cancel --cleanup` (:6397); `koto session cleanup` (cli/session.rs:769, cli/mod.rs:1371-1374); `koto workspace prune` (cli/workspace.rs:183-189); scheduler half-init repair (cli/batch.rs:668) and reclassification (batch.rs:938) inside `koto next`; retry respawn (cli/retry.rs:579, 638) |
| `migrate_if_needed`, session/local.rs:738-845 | **rename** old-layout session dirs to the flat level (:815) or to quarantine (:796) | n/a | no | every `LocalBackend::new()` (local.rs:43), i.e. every command that builds a backend. Only touches 16-hex-named dirs |
| init-inline failure cleanup, cli/init_child.rs:847-850 | `remove_dir_all` on the session dir | n/a | no | `koto init --from-stdin` when compile fails; no log exists at that point |

### Cloud backend (session/cloud.rs), which wraps LocalBackend

| Path | Operation | Creates if missing | Lock | Reached from |
|---|---|---|---|---|
| `CloudBackend::append_event`, cloud.rs:729-738 | local append, then `sync_push_state` (:121-146): read the whole local file and PUT it; errors are only warnings | as local | no | every backend append under `session.backend = "cloud"` |
| `sync_pull_state`, cloud.rs:150-164 | **`fs::write` the remote bytes over the local log** (:155): truncate + write, no temp, no lock | creates the file if the dir exists (no `create_dir_all`) | no | **every `read_events` (:747) and `read_header` (:752)**, so every verb that reads a session, including `koto next`, `status`, `rewind`, `cancel`, `rebind` |
| `write_local_state_bytes`, cloud.rs:542-548 | `create_dir_all` + `fs::write` | yes (dir and file) | no | `koto session resolve --children accept-remote/auto` (cli/session.rs:810, 898 → cloud.rs:519-521) |
| `force_pull_session`, cloud.rs:618-657 | `create_dir_all` + `fs::write` of the log (:627) | yes | no | `koto session resolve --keep remote` (cli/session.rs:808 → cloud.rs:359-361) |
| `CloudBackend::init_state_file` / `relocate` / `cleanup`, cloud.rs:756-778, 780-841, 694-698 | delegate to local, then best-effort S3 push/copy/delete | as local | no | as local |
| `CloudBackend::lock_state_file`, cloud.rs:843-854 | delegates to local; per-host only | n/a | as local | as local |

### CLI verbs that append through `backend.append_event`

| Verb / site | Existence guard before the write | Lock |
|---|---|---|
| `koto context add` (cli/context.rs:45) | **none**. `ContextStore::add` runs first and `create_dir_all`s `<base>/<id>/ctx` (session/local.rs:497), which recreates a missing session dir | no |
| `koto context remove` (cli/context.rs:147) | **none**. `remove` does not create dirs (local.rs:563-600), so a missing dir gives ENOENT; a dir with no log gets a new headerless log | no |
| `koto next`: adoption event (cli/mod.rs:3600) + header rename-replace (:3613) | `exists` at :3485 | no (before the lock) |
| `koto next --to`: `DirectedTransition` (:3846), `InstructionsDelivered` (:3982), terminal tick (:3994), then `exit(0)` (:4003) | yes | no (exits before the lock) |
| `koto next --with-data` `retry_failed`: parent appends (cli/retry.rs:410, 428), child `Rewound` (retry.rs:556), child cleanup + re-init (retry.rs:579-660) | yes, for the parent | no (dispatched at cli/mod.rs:4206, before the lock) |
| `koto next --with-data` `EvidenceSubmitted` (:4341) | yes | no (before the lock) |
| `koto next` advance loop (closure :4427-4431, used at engine/advance.rs:364, 923, 1051, 1093), `DefaultActionExecuted` (:4655), `SchedulerRan` (:5232), `BatchFinalized` (:5285), `InstructionsDelivered` (:5437) | yes | **conditional**: held only when the tick started on a batch-scoped state |
| terminal tick `finish_terminal_tick` (cli/mod.rs:2668-2728): child `RequestStoreResult` (:2432), **parent `ChildCompleted` (:2561)**, abandonment-notice audit (:2997 via :2947), then child cleanup (:2724) | parent: `exists` at :2532 (TOCTOU) | the child's lock at most, **never the parent's** |
| `koto next` startup wake pass: `emit_one_wake_batch` → `persistence::append_event` on the coordinator log (engine/wake.rs:336) at a hardcoded `~/.koto/sessions` path (cli/mod.rs:3407-3423) | `coord_state_path.exists()` (:3411), TOCTOU | no. Also bypasses the backend (no cloud push) and runs before every other check in `handle_next` |
| `koto rewind` (cli/mod.rs:2188), then relocate children (:2209) | `exists` :2128 | no |
| `koto cancel` (cli/mod.rs:6384) | `exists` :6294 | no |
| `koto overrides record` (cli/overrides.rs:286) | reads events first | no |
| `koto decisions record` (cli/mod.rs:5700) | `exists` :5533, `read_events` :5540 | no |
| `koto session rebind` (cli/session.rs:577) + rename-replace (:581) | `exists` :526 | no |
| `koto session update` → direct `persistence::append_event` (cli/session.rs:494), also called by the init flows for `--intent` (cli/mod.rs:1803, 1971, 2087) | `exists` :486 | no. Bypasses the backend (no cloud push, no `/workflows` materialize) |

### Written but unreachable in production

`claim_and_dispatch` (engine/claim.rs:642), `recover_orphaned_sidecar` (claim.rs:494) and
`execute_respawn` (engine/respawn.rs:421) have no callers anywhere in `src/` outside their
own definitions. That makes their writes dead code today: `rewrite_header_atomically` at
claim.rs:432 and :569, `append_event` at claim.rs:626 and :677, and `append_event` at
respawn.rs:548 and :569. The same goes for their `create_dir_all` of the coordinator log's
parent (claim.rs:623, :674) and the sidecar `create_dir_all` (claim.rs:246).

## Findings

**The lock's single production caller is confirmed.** `backend.lock_state_file(&name)` at
cli/mod.rs:4377 is the only non-test call. Two conditions gate it. It needs `#[cfg(unix)]`.
And `state_is_batch_scoped(&compiled, current_state, &events)` has to be true
(cli/mod.rs:2348-2351). That function just returns
`batch::state_has_materialize_children(compiled, state_name)`, and it ignores events (the
event-based check is still a TODO at :2343). `current_state` is the state *before* the
advance loop runs. The doc comment at :2323-2327 still says the function "currently returns
`false` in every case", which is stale. The guard is `_batch_lock` (:4375), which lives until
`handle_next` returns. It covers the advance loop, the scheduler, and the natural-path
terminal tick. It does not cover anything earlier in the function: the startup wake-pass
append to the coordinator log (:3416), the anchor-adoption append and rename-replace
(:3600, :3613), the whole `--to` path, which exits at :4003, the `retry_failed` dispatch
(:4206), or the `--with-data` evidence append (:4341).

**What the lock is taken on.** It's an flock on the log file itself, not a sidecar. The
open is read-only with no create flag (session/local.rs:353-356), so it fails with
NotFound on a missing log. It's non-blocking (`LOCK_EX | LOCK_NB`, :364), and `EWOULDBLOCK`
maps to `SessionError::Locked` (:367-373), which `koto next` turns into
`BatchError::ConcurrentTick` (cli/mod.rs:4379-4382). The other flock,
`acquire_state_flock`, also locks the log inode but is blocking, and it never touches a
session state log. So no state-log writer outside the batch advance loop is excluded by
anything. And because the lock sits on the inode, `rewrite_header_atomically`'s rename
(engine/claim.rs:418) swaps in a new inode. A lock held on the old one excludes nobody who
opens the path afterwards. An appender that opened the old inode before the rename and
writes after it loses its line into the unlinked file.

**Every append creates the file if it's missing.** None of them checks for a header. The
open at engine/persistence.rs:159-160 is `create(true).append(true)`. It creates the file
but not the directory, so an append to a session whose directory is gone fails with ENOENT.
Only `context add` recreates the directory: `ContextStore::add` runs `create_dir_all` on
`<id>/ctx` (session/local.rs:497) before the append at cli/context.rs:45, and there's no
`exists` check anywhere on that path. That is #236's mechanism, and it holds in three
situations:

- a directory that exists with no log in it;
- a session auto-cleaned after reaching a terminal state (cli/mod.rs:2724);
- a child that `koto rewind` moved away (session/local.rs:326).

In all three, the append writes a one-line `context_added` log at seq 1. The next append
reads that log with `skip(1)` (engine/persistence.rs:518), finds no events, and writes seq 1
again. That's the duplicate `seq:1` from the brief.

**Four paths rewrite the whole log.** None takes the lock.

- `rewrite_header_identity` truncates and writes in place (session/local.rs:682).
- `rewrite_header_atomically` reads, writes a temp file, and renames it over the log
  (engine/claim.rs:374-418).
- `sync_pull_state` in the cloud backend truncates and writes in place on every read
  (session/cloud.rs:155).
- `write_local_state_bytes` and `force_pull_session` do the same during `session resolve`
  (cloud.rs:546, :627).

Any of them loses an event that another process appends between its read and its
write or rename. The two in-place truncations also leave a window where readers see an empty
file, which `read_header` reports as "state file is empty" (engine/persistence.rs:552).

## Implications

"Lock coverage for every writer" is a big change. Today the lock covers one conditional
block of one verb, and at least six unlocked paths write the logs of batch parents. Four are
in `koto next` itself:

- evidence submission;
- `retry_failed`;
- `--to`;
- the terminal tick on a child, which appends `ChildCompleted` to the parent at
  cli/mod.rs:2561.

The other two are `context add` and `rebind`. The cross-session one matters most: a child's
terminal tick writes to the parent's log without the parent's lock. That's exactly the
concurrent-writer shape the batch design cares about, and it lives in a different
process from the parent's `koto next`.

Because the lock is taken on the log inode, whole-file rewrites have two options. They can
take the lock and rewrite in place (truncate and write under the lock). Or the lock can move
to a stable sidecar file (for example `<id>/state.lock`), which lets temp-and-rename stay
safe. Keeping rename-replace while locking the log inode doesn't protect anything. The
non-blocking semantics also matter to #171. If ordinary appends start taking this lock,
fail-fast would turn contention between agents into user-visible errors on common verbs like
`context add`. A blocking lock, or a separate blocking append lock, fits appends better.

The cheapest fix for #236 is also the most targeted one. Refuse to append when the log
is missing. `persistence::append_event` could drop `create(true)`, or check that line 1
parses as a header. That closes the headerless-creation route for every verb at once,
including the ENOENT-free `context add` case after cleanup or relocate. Only
`init_state_file` needs to create logs, and it has its own path.

For #200: the brief's relocate theory holds, but it isn't limited to relocate. #200
describes "a log truncated to one `context_added` line", and that is exactly what a missing
log plus `context add` produces, whether the log vanished through cleanup or through
relocate. A truncation never has to happen. I'd reproduce with `rewind` on a batch parent
(or a terminal auto-clean) followed by `koto context add <old-child-name>` before trying
truly concurrent appenders.

## Surprises

1. **A cloud read overwrites the local log.** Under `session.backend = "cloud"`, every
   `read_events` and `read_header` replaces the local log with the S3 copy, with no temp
   file and no lock (session/cloud.rs:150-164, 747, 752). Every append pushes the whole local
   file (cloud.rs:121-146, 735-736). So a headerless one-line log that `context add` creates
   on one host gets PUT over a healthy remote log. Any later read on another host then
   writes it over that host's good local copy. `CloudBackend::exists` also returns true for
   sessions that exist only on S3 (cloud.rs:686-692), so existence guards pass even when the
   local directory is missing.
2. **`append_header` isn't an append.** It opens with `write(true)` and no `append`
   (engine/persistence.rs:91), so on an existing file it overwrites the first bytes at
   offset 0. It has no production caller, but a recovery fix that reached for it to "put the
   header back" would clobber the first event instead of prepending.
3. **The wake pass writes before anything is checked.** Every `koto next` starts with a
   wake-candidates pass that appends to the coordinator's log through a hardcoded
   `~/.koto/sessions` path (cli/mod.rs:3407-3423, engine/wake.rs:336). It skips the backend
   (no cloud push), runs before the nested-tick and existence checks that come after it, and
   takes no lock.
4. **The whole claim and respawn protocol is dead code.** `claim_and_dispatch`,
   `recover_orphaned_sidecar` and `execute_respawn` have no production callers, so their
   rename-replace header writes and their appends are unreachable today. That shrinks the
   live inventory. It also means turning them on later would add more unlocked rename-replace
   writers.
5. **`rewrite_header_atomically` leaves temp files nothing sweeps.** Its temp name is
   `.<logname>.tmp.<pid>` (engine/claim.rs:388-392), which ends in `.tmp.<pid>`, not `.tmp`.
   The scheduler's sweep matches `.koto-*.tmp` (cli/batch.rs:712), so a crash mid-rewrite
   leaves the temp file behind for good.
6. **`session update` bypasses the backend.** It calls `persistence::append_event`
   directly (cli/session.rs:494), so under the cloud backend an intent update is never
   pushed and doesn't trigger `/workflows` materialization.
7. **The "batch-scoped" check looks at the pre-advance state.** If a tick starts outside a
   batch state and advances into one, the scheduler runs unlocked. If it starts in one and
   leaves, it holds the lock for the rest of the tick.

## Open Questions

- Do the rename-replace header writes in `rebind` and anchor adoption actually lose events
  in practice? A test that appends from a second process between `read_to_string` and
  `rename` would settle it. This belongs to the #200 reproduction.
- Should `koto context remove` also refuse when the log is missing? Its directory
  behaviour differs from `add` (no `create_dir_all`), but it can still create a
  headerless log in a directory that has no log.
- Where should a lock-covered rewrite lock: the log inode (rewrite in place under the lock)
  or a new sidecar? That's the lock-semantics lead's call. This inventory only shows that
  rename-replace plus an inode lock can't work.
- Is the cloud pull-overwrite in scope for this PR, or a separate issue? It's the most
  likely way corruption spreads across machines, but it's a different kind of fix (compare
  seq or length before overwriting).
- I didn't trace which verb each init-time `handle_update` call (cli/mod.rs:1803, 1971,
  2087) belongs to beyond "the init flows". They're guarded by `exists`, so they don't
  change the headerless analysis.

## Summary

Every state-log append goes through `persistence::append_event`, which uses `create(true)`
and never checks for a header. `context add` alone recreates a session directory that
cleanup or relocate removed, and that fully explains both #236 and the one-`context_added`-line
shape in #200. Four paths rewrite the whole log with no lock at all:

- the truncate-and-write in `rewrite_header_identity`, used by relocate and recover;
- the temp-and-rename in `rewrite_header_atomically`, used by rebind and anchor adoption;
- the cloud backend's `sync_pull_state`, which runs on every read;
- the cloud backend's resolve pulls (`write_local_state_bytes` and `force_pull_session`).

The brief's claim holds: the only lock is a non-blocking flock on the log inode at
cli/mod.rs:4377. It's taken only when `koto next` starts on a state with a
`materialize_children` hook, and only after the adoption, `--to`, `retry_failed` and
evidence writes, so it excludes almost none of the writers, including a child's unlocked
`ChildCompleted` append to its parent.
