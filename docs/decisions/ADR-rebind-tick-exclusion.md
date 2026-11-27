---
status: Proposed
decision: |
  Each session directory gains a never-renamed `tick.lock`. Every top-level
  `koto next` holds it shared from before its first write to process exit,
  and `koto session rebind --whole` holds it exclusive for the whole move,
  waiting up to `--wait` (10 seconds by default, 0 refuses at once) before
  refusing `session_busy`. A strengthened write fence underneath refuses
  writers outside a tick once the move has landed.
rationale: |
  Only a lock a tick holds across its own lifetime can promise that a tick
  lands wholly before the move or not at all. The state file is replaced by
  rename on every header rewrite, so the lock has to live on a file that never
  moves, and it has to be shared so ordinary ticks keep running side by side.
  The fence stays for writers a tick lock can't hold off.
---

# ADR: rebind tick exclusion

## Status

Proposed

## Context

`koto session rebind <name> --whole` moves a run from one agent session to another. It moves the execution directory, the origin anchor (advancing its epoch and retiring the old anchor), the held-by links, the publish location and the command environment, logs each part, and finishes on a re-run. The ask is that a tick from the old anchor either finish before the move or be refused after it, never land in between. Today a plain `koto next` takes the state-file lock only on a batch-scoped state, so a rebind has nothing to wait on and nothing to exclude.

The state-file lock can't be stretched to cover this. Every header rewrite and every cloud pull replaces the state file by rename (`replace_log_bytes` in `src/engine/persistence/append.rs`), so a flock on the state file ends up on an orphaned inode once the move's own header rewrite lands. A plain tick also writes before its anchor check: the held-by write in the `Command::Next` arm, the wake pass's `requester_woken` append, and decision repair all run before the check in `handle_next` (`src/cli/mod.rs`). So the exclusion has to be in place before a tick's first write, on a file that is never renamed. Koto already keeps sidecar locks like that: `environment.lock`, `decider.lock`, `append.lock`, `cloud-batch.lock`.

## Assumptions

- `flock` shared and exclusive locks behave the same on every platform koto supports, and a shared holder never blocks another shared holder. If a platform differs, concurrent plain ticks would serialize there.
- Taking one more flock per tick costs little next to the state-file read a tick already does. Nobody has measured it.
- Snapshot or listing tests that enumerate a session directory's files tolerate one more sidecar file, or are updated with it.
- A `koto next` that receives SIGTERM keeps holding the lock until its running command returns, since the flag is checked only between transitions and in polling waits. A run would need to confirm this.
- Ticks run on the same host as the rebind. A cloud-backed session ticked from a second host isn't covered by a host-local lock (see Consequences).

## Decision

**Sidecar tick lock, shared per tick and exclusive for the move, over a strengthened write fence.**

1. Each session directory gains a `tick.lock` file that is never renamed or replaced. Every top-level `koto next` takes it shared at the top of the `Command::Next` arm, before its first read of the session (the superseded pre-check), the held-by write, the wake pass and decision repair, and keeps the descriptor until the process exits, so the post-tick cloud flush is covered too. Every read the fence relies on, the anchor epoch included, happens after the lock is held. A tick from the new anchor that waited out a move therefore sees the new epoch rather than being refused. Under the lock, the tick reads the header and runs the anchor check before it writes anything. Batch-scoped ticks still take the state-file lock exclusive and still refuse `ConcurrentTick`. That's a separate file, so nothing gets converted from shared to exclusive. Concurrent plain ticks still run side by side.
2. `rebind --whole` takes `tick.lock` exclusive before it reads or writes anything. It retries a non-blocking attempt every 50 ms for up to `--wait <secs>`, which defaults to 10 seconds the way pin's state-file wait does, and `--wait 0` refuses at once. Past the bound it refuses `session_busy` with exit 1, the retryable class `concurrent_access` already uses, writes nothing, and says a tick is in flight. Rebind's existing `background_run_in_flight` refusal exits 2 and stays as it is. The success output gains an additive field saying whether the move had to wait. There is no writer-preference turnstile. A bounded poll can't wait forever, and a starved move ends in the same `session_busy`.
3. Holding `tick.lock` exclusive, the move checks for an in-flight background run, so it refuses `background_run_in_flight` for a runner a tick started before logging `background_started`. Then it does every part. It takes `ctx/manifest.lock` and then the append lock, both with bounded waits, across the commit point only: the move event plus the header rewrite, written through the held variants (`append_event_tracked_held`, a held header rewrite on each backend). Every later part (the held-by link changes, the publish-location key, environment adoption, the cloud context carry) takes its own locks after the commit point releases them. Holding the append lock across those parts would deadlock, because taking it a second time in the same process never returns. The intended order is `tick.lock`, then the state-file lock, then the manifest lock, then the append lock. Two existing paths break it. `koto context add` holds the append lock and then waits on the manifest lock with no bound (`src/session/local.rs`). That's the reverse of what decision writers do, and decision repair inside a tick waits on both with no bound, so the two can deadlock today. Under this design a tick stuck that way would also hold `tick.lock` and keep every move refused. The cloud deferred header rewrite takes the append lock and then `cloud-batch.lock`, which the order above leaves out. Context add's order is fixed to manifest-then-append before this ships, and `cloud-batch.lock` is placed after the append lock in the documented order.
   The mover's own later writes have to pass the fence it just raised. The mover adopts the epoch it wrote at the commit point, as the cloud backend already does when it re-reads after its own header rewrite. Its writes are checked against the new anchor, not the retired one, even when the move runs from the old directory, which rebind allows. Without that, the mover's held-by, publish-location and environment writes would be refused, and every re-run from the old directory would refuse the same way.
4. A tick that arrives while the move holds the lock waits on its shared take, retrying for up to 30 seconds. It then re-reads the header and is refused before writing anything: `execution_anchor_mismatch` when its directory lies outside the new anchor, and the fence's retired-anchor refusal when its directory lies inside the new anchor and under the retired one. Past 30 seconds it refuses `concurrent_access`.
5. Any verb that takes `tick.lock` exclusive is refused up front when `KOTO_TICK_SESSION` names the same session, since it would wait on its own tick. `koto session import` probes `tick.lock` exclusive and non-blocking, and refuses `import_source_running` while a tick holds it. Plain `koto session rebind` takes no `tick.lock`: it stays exactly as it is, including running from a gate on its own session. A tick's own first-tick anchor adoption changes the execution directory too, but it takes no exclusive lock. Pin, prune and reclaim are unchanged here.
6. A write fence goes underneath for writers that aren't ticks: context add, cancel, rewind, decision writes, and commands a gate started that outlive their tick. The check sits in `append_under_lock`, which also covers the wake pass, and in header rewrites and local context writes. A local context write takes the manifest lock before writing its content file, so a refused write leaves nothing behind. A process keeps the anchor epoch from its first read, taken under `tick.lock` for a tick; the cloud backend refreshes it on every read today. The one exception is the mover, as point 3 says. The retired-anchor rule matches subdirectories of a retired anchor as well as the anchor itself; today it matches only the exact directory. When a directory lies under both a retired anchor and the current one, the deeper of the two decides. A move to a parent directory therefore still refuses a writer in the old directory, and a move into a subdirectory admits writers there. The fence applies on both backends.

**Amendment: what turns the fence on, and the local fence.** The strengthened fence in point 6 is gated on the session's own header. It applies only once a whole-binding move has written `origin.whole_epoch`, the `anchor_epoch` that move recorded. A session no whole move has touched keeps plain rebind's fence exactly as it is: cloud backend only, by prefix, with the exact retired anchor. Applying the epoch and subdirectory rules to every session was tried and dropped. On the local backend it refuses writers that race a header rewrite, which the context-append tests pin as losing nothing, and it would tighten plain rebind's cloud fence, which this decision leaves alone. Under the gate, the fence is new on the local backend. It reads the process's canonical directory once and keeps the first epoch the process read of each log, so a later re-read doesn't lift the fence. The cloud backend keeps its own per-read epoch, as before. A tick that arrives from under a retired anchor deeper than the current one is refused with a new `koto next` error code, `session_rebound` (exit 2), before it writes. A `context_read` append passes the fence, as it passes the superseded fence: it records a read, and reads keep working wherever they run.

Three details of how the tick takes the lock and moves its writes aren't in the points above. The cloud backend's tick id used to be written by `begin_tick` before the tick's first read. Moving that write behind the anchor check would have let the first read find the previous tick's id in `cloud-tick.id` and count its stamp as already checked. So the tick now picks its id in memory at the old point, every read uses it, and `begin_tick` writes that same id once the anchor check passes. A refused tick leaves no `cloud-tick.id` behind. A cloud session that the tick's first read has just pulled had no directory when the lock was first tried, so the lock is offered again right after that read. And when `tick.lock` can't be opened or taken for a reason other than another holder (an unreadable directory, say), the tick runs without it, as ticks did before the lock existed. It doesn't refuse. The exclusion this decision promises holds only while the lock can be taken.

## Rationale

The ask is about one tick: it lands wholly before the move or not at all. Only a lock that a tick holds across its own lifetime can promise that. A fence by itself refuses the first write after the move but lets a tick straddle it: some events land before, the rest are refused, and gates and actions have already run in the old directory. An unlogged action, like a push or a background runner started before it was logged, could then be repeated by the successor. The lock has to sit on a file that never moves, because the move itself replaces the state file. It has to be shared, so ordinary ticks keep running side by side as they do today and the existing test that a non-batch tick ignores an external flock on the state file still holds. The fence stays, because writers outside a tick can't be held off by a tick lock. For them the after-the-move half of the promise is the one that can be kept.

The default wait was the one point the review left split. Safety is the same either way; the difference is whether a move that meets a predecessor's last, finishing tick goes through or returns `session_busy`. A 10-second bounded poll matches pin, which already waits that long on a lock batch ticks hold across template commands. It turns the common takeover case into a success and keeps `--wait 0` for a caller that wants to refuse at once. Callers still have to handle `session_busy`, since a polling tick has no upper bound.

## Options Considered

### Shared/exclusive flock on the state file

Every tick holds the state file shared and the move holds it exclusive, with a batch tick converting shared to exclusive. Rejected because the state file is replaced by rename on every header rewrite and cloud pull, so the lock falls off mid-move. It would also contradict the test that a non-batch tick ignores an external flock, make terminal ticks contend with their own self-removal, and the conversion from shared to exclusive isn't atomic.

### Write fence only

The move holds its existing locks around its own writes, and every later write from the old anchor or an older epoch is refused. Rejected because a tick that straddles the move is exactly the "in between" the ask rules out. It's kept as the layer under the chosen option.

### Sidecar tick lock with a drain and an entry turnstile

The chosen shape with a second sidecar that gives the move writer preference over new ticks. Rejected because a bounded poll can't wait forever, so the turnstile adds a lock to every tick's start and only helps the move go through sooner.

### Sidecar tick lock as a probe that refuses at once

The chosen shape with no wait at all. Not rejected so much as folded in: it's what `--wait 0` does. It lost on the default because it hands every caller a retry for the most common case.

## Consequences

For ticks on the same host from builds that take the lock, the tick question is solved: a tick in flight finishes before the move, and one that arrives during or after it is refused before it writes. It isn't fully solved, though. These residuals remain:

- **Cross-host ticks on the cloud backend.** The lock is host-local, so a tick on a second host is ordered against the move only by the conditional push. The loser of that race may be the move. Closing this needs either a cloud-side lease or an explicit statement that cross-host takeover is out of scope.
- **Ticks from older builds.** A tick already running under a binary from before the change takes no lock.
- **Writers outside a tick.** They're refused after the move rather than held off. A verb that makes several writes can land some before the move, with the rest refused.
- **Commands that outlive their tick.** A gate's detached descendants, or the command group of a tick killed with SIGKILL, keep running in the old directory until they finish or time out.
- **A tick that never stops.** A predecessor that keeps ticking keeps the move refused. Stopping it is the caller's job, not koto's.
- **A move into a subdirectory of the old anchor.** The old agent, working in that subdirectory, passes both the anchor check and the deeper-wins retired-anchor rule. Only the lock keeps its ticks out, so its writers outside a tick there aren't refused.

Every tick takes one more lock, and every session directory gains one file. Moves become all-or-nothing as far as ticks are concerned, which makes a whole-binding takeover safe to retry. Fixing `koto context add`'s lock order, which today can deadlock against decision repair, is a prerequisite for this change, not a follow-up. Prune and reclaim not skipping a session in the middle of a tick is left for a separate issue.
