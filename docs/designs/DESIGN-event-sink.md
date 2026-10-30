---
schema: design/v1
---

# DESIGN: An external event sink for koto's session records

## Status

Proposed

## Context and Problem Statement

koto writes one run-journal line per session event to `~/.koto/_run_journal.jsonl`,
and a separate host job later copies that file and forwards it. That hop is
being removed. koto will hand its records to an external event sink, a separate
executable at a fixed path, at defined points in each command
(tsukumogami/koto#544). The journal keeps running unchanged in this release;
release B removes it (tsukumogami/koto#545).

The sink's contract is fixed upstream and isn't reopened here: discovery at
`$HOME/.local/libexec/fleet/event-sink` with owner, mode, symlink and
parent-directory checks; `FLEET_EVENT_SINK=off` as the only switch; the call
`event-sink send --scope koto-journal [--carry N]` with NDJSON on stdin; exit
codes 0, 1, 2, 3 and 4; at most 200 records a call; one 3.0 s deadline per
flush with a process-group kill; a circuit breaker on exit 3, a kill or exit 2;
and five flush points. What this design settles is how koto does it: where a
journal write becomes an enqueue, the exact code site of each flush point, how
the deadline and kill are built, the carry sidecar and its lock, the exit-hook
list and the `exit(code)` helper, the stamps on the queued copy, the module
moves, `koto session identity`, the `run_journal.enabled` key, the stub-sink
test harness, and which parts ship in release A and which in release B.

Today every record goes through one function, `run_journal::write`
(`write_within` for the `response` record), called from five hooks:
`after_init`, `after_commit`, `terminal`, `response` and `imported`. Commands
leave through about two dozen direct `std::process::exit` calls and one
`OnceLock` exit flush (`EXIT_FLUSH` in `src/cli/mod.rs`) that `koto next` and
the `--workspace` read path each set for their own cleanup.

## Decision Drivers

- The sink contract is fixed: discovery, call shape, exit codes, the 200-record
  chunk, the 3.0 s deadline and the circuit breaker are designed against, not
  revisited.
- Telemetry never fails, retries or blocks a command past the 3.0 s bound, and
  no flush sits inside an action's 30 s timer.
- The journal stays byte-for-byte as today in release A; the queued copy alone
  carries the new stamps.
- Inert until install: with no sink at the fixed path koto makes no call and
  does no extra work beyond one discovery check per process. Release A is cut
  before 2026-11-09 and installed on hosts only after 2026-11-09T02:00Z.
- `context_reads` records never reach the sink.
- The release date is tight, so the build reuses what koto has (the record
  builders, the process-group kill in `src/action.rs`, the existing session
  sidecar) rather than adding new machinery.
- Tests never reach a real sink: every test that exercises sending does so
  through a stub at the fixed path under a scratch HOME.

## Considered Options

### Decision 1: Where a write becomes an enqueue, and how the sink is called

`write` and `write_within` see only the journal root and the records. They
can't stamp the run template or say which session a dropped record belongs
to, and each hook returns early when the store has no journal root, which
would also stop sending once the journal is turned off. The hooks hold the
session, the backend and the header. A 200-record chunk can reach about
800 KB, more than a pipe buffer, so the thread that watches the deadline
can't also be the one writing stdin.

Key assumptions:

- A spawn failure or a wait error counts as a kill: the chunk is dropped and
  counted, and the breaker trips.
- Exit 1, and any code outside the contract, neither retries nor trips the
  breaker.
- A stamped line over the 4096-byte line cap isn't sent and is counted as
  dropped.

#### Chosen: one widened choke point, a process-global queue, discovery once per process

`write` and `write_within` become one `emit(ctx, records, lock_wait)` that all
six call sites use (the five hooks, plus `response`'s 30 ms path). `ctx`
carries the journal root (`None` when the store has none or
`run_journal.enabled` is false), the session directory, and the run template
name and hash. `emit` enqueues first, then writes the journal from the
unstamped records exactly as today, so the journal lock's outcome never
touches the queue and the journal's bytes don't change. Each hook returns
early only when there's no journal root and the sender is inactive.

The queue is a `static Mutex<Queue>` with a `cfg(test)` thread-local twin,
the pattern `GAVE_UP` already uses. Discovery runs once per process, on the
first enqueue, into a `OnceLock<Option<SinkPath>>`. When it finds no sink,
`emit` returns before any clone or serialization, so release A costs one
cached check per process until a sink is installed. The sink runs as its
own process group with stdin fed by a writer thread, and the flushing thread
waits with `wait_timeout`, the same crate and group-kill code
`src/action.rs` uses. The breaker is a process-global atomic.

#### Alternatives Considered

**Enqueue inside `write`/`write_within` with their current signatures.** One
choke point with no signature change. Rejected: they have no session or
template, so each hook would have to set a thread-local "current session"
first, which is the same per-hook coupling, just hidden. The `journal_root`
guard would still block sending with the journal off.

**An explicit enqueue call at each hook.** The context is already in hand
there. Rejected: six sites would each repeat the `context_reads` filter, the
line cap and the stamping, and a future hook could journal without sending,
or send a `context_reads` record, with nothing structural to stop it.

**A tee that re-reads the journal file.** Leaves the write path alone.
Rejected: it can't send with `run_journal.enabled=false`, journal lines lack
the queued-copy stamps, other processes' lines interleave, and rotation
moves offsets under it.

**A per-backend or thread-local queue.** Rejected: flush point 1 and the
exit hook hold no backend, and import and takeover touch two. A thread-local
queue hides other threads' records; it survives only as the test twin.

**Discovery at every flush, or a helper thread with `recv_timeout`.**
Rejected: the first repeats the stat walk for nothing (the ownership checks
already close the swap window), and the second adds a thread and a channel
for what `wait_timeout` already does.

### Decision 2: The carry sidecar and its lock

Before a send, the flush takes each queued session's `dropped` count, writes
0 back and passes the sum as `--carry N`; after exit 3 or a kill it adds the
taken counts back. Two processes must never carry the same count, and the
whole flush stays inside 3.0 s. The terminal tick's flush runs just before
the session directory is removed, so a count added back then would go with
the directory.

Key assumptions:

- The carry is unix-only. Off unix there's no `flock`, so the carry is off
  (no take, no `--carry`) rather than unlocked.
- Undercounting drops (a crash, a lock that times out on add-back) is
  acceptable; carrying a count twice is not.

#### Chosen: a separate lock file, rename writes, a 50 ms bounded wait, and an orphan count in the store

`<session dir>/telemetry.json` holds `{"dropped": N}` and is only ever
replaced by tempfile and rename, mode 0600, the way the `run-journal.json`
sidecar is written. Mutual exclusion comes from a data-free
`<session dir>/telemetry.lock` beside it, opened `O_NOFOLLOW` and 0600, never
deleted, and locked through `lock_exclusive` in
`src/engine/jsonl_append.rs`, the call `AppendLock::acquire_within` already
makes on `append.lock`. A take is lock, read, write 0, unlock; an add-back is
lock, read, saturating add, write, unlock. Each lock waits at most 50 ms from
the flush's budget, with no unlocked fallback: a take that can't lock skips
that session and leaves its count for the next flush. A per-process gave-up
flag, like the journal's, means one stalled holder costs one wait. A file
that isn't a regular file, is over 4 KiB, or doesn't parse as an object
whose `dropped` is a non-negative integer reads as 0, and the next take
heals it.

Just before a session directory is removed (`LocalBackend::cleanup` and
prune's direct removal, through one helper), the remover takes the session's
remaining count and adds it to `_telemetry_orphans.json` in the session
store's base directory, under `_telemetry_orphans.lock`. That's the parent of
every session directory, which session listing already skips because it
holds only plain files there. Every flush takes the orphan count of each
store its queue touches, alongside the sessions' own.

#### Alternatives Considered

**`flock` on `telemetry.json` itself, rewritten in place.** One file and no
double carry. Rejected: nothing in koto locks a file and then truncates it in
place, correctness would depend on nobody ever switching the write to the
rename pattern every other sidecar uses, and a crash between write and
truncate leaves a torn file.

**An `O_EXCL` lock file as the mutex.** Rejected: a crash leaves a stale lock,
and breaking a stale lock by age or pid is exactly how two holders end up in
the take together.

**A non-blocking try with no wait.** Rejected: any overlap of two flushes
skips sessions, and at the terminal flush a skipped take strands its count.

**Accept the loss at removal, or keep the directory while `dropped > 0`.**
The first is simplest but loses the count of every session that finishes
during a sink outage, which is when the count matters most. The second
changes session lifetime to serve a counter. Both rejected.

**Put the orphan count in the koto home beside the journal.** That's where
the decider first placed it. Rejected in cross-validation: release B
requires koto to write nothing under `~/.koto` outside the session store.

### Decision 3: The exit-hook list and the `exit(code)` helper

`EXIT_FLUSH` holds one callback: `koto next` sets it to push deferred cloud
writes, and the `--workspace` read path sets it to remove a scratch
directory. About two dozen direct `std::process::exit` calls skip it, and
`main`'s normal return runs nothing. CI already requires
`cargo clippy --all-targets -- -D warnings`, and `clippy.toml` already bans
two functions through `disallowed-methods`.

Key assumptions:

- `crash_at` in `src/session/cloud_takeover.rs` exits 86 to simulate a killed
  process, so it gets a no-hooks sibling `crash(code)`; running hooks there
  would defeat the simulation.
- Panics stay unhooked (default exit 101), so records queued before a panic
  are lost.

#### Chosen: a registered list in `src/exit.rs`, run in reverse, with `main` ending in `exit(code)`

`src/exit.rs` holds `static HOOKS: Mutex<Vec<Box<dyn FnOnce() + Send>>>`,
`register`, `exit(code) -> !` and `crash(code) -> !`. `exit` takes the list
out of the mutex (so a hook that calls `exit` again finds it empty), runs the
hooks in reverse registration order, each under `catch_unwind`, and then
exits. `main` registers the telemetry flush first, so it always runs last,
and ends in `koto::exit::exit(code)` for both its `Ok` and `Err` arms. The
`next` arm and `workspace_read` call `register` in place of
`EXIT_FLUSH.set`; `EXIT_FLUSH`, `flush_before_exit` and the
`workspace_read` assert go, along with the explicit flush on `next`'s normal
return, which `main`'s `exit(0)` now runs.

`clippy.toml` bans `std::process::exit`, and the helper's one raw call is a
`#[rustfmt::skip]` one-line `fn exit_now` with a local `allow`, so the
issue's `grep -v 'fn exit'` check also passes. The two exits in `benches/`
get a local `allow`, since `--all-targets` lints them. Comments that quote
the literal are reworded.

#### Alternatives Considered

**Fixed named hook slots.** Makes order explicit. Rejected: it ties
`src/exit.rs` to every subsystem that owns a hook, and only partly meets the
list the issue asks for. A test on registration order gives the same
guarantee.

**libc `atexit`.** Rejected: unsafe code, runs after `main` when stdio and
thread-locals may be torn down, and fires on `crash`'s exit 86 too.

**A drop guard in `main`.** Rejected: it runs hooks during panic unwinding,
where a hook that panics aborts the process and the cloud flush could push
half-finished state.

**A source-scan test as the gate.** Rejected as the gate: it matches comments
and strings and misses renamed imports. Clippy resolves the real call path.

### Decision 4: The run template and `koto session identity`

Every queued record carries the template of the run's root session. Nothing
resolves that today. A modern child records the root's id, but nothing maps
an id back to a session name, so reaching the root's header needs the old
`parent_workflow` walk, which breaks once an intermediate ancestor is
cleaned up. The run template is cheap to find only at child creation, when
`child_lineage` already reads the parent's header and the parent is local.

Key assumptions:

- A root created by `koto init` has a non-empty template hash; an empty one
  is left out, as `session_started` does today.
- The four `sidecar::write` sites are the only writers of `run-journal.json`.

#### Chosen: cache it in the `run-journal.json` sidecar; `identity` reads locally and writes nothing

A resolver beside `run_id` returns the run template name and hash, each
optional. A root answers from its own header. A child reads two new
optional sidecar fields, `run_template_name` and `run_template_hash`
(`skip_serializing_if = "Option::is_none"`, so an existing sidecar keeps its
bytes, shape-checked on read), which `after_init` fills from the parent's
header or the parent's sidecar in the same write that caches `run_id`. When
the cache is empty, a walk of local headers accepts the top header's
template only if its `session_id` equals the run id, and `cached_run_id`
stores what it finds. Otherwise both stamps are left out, never guessed.

`koto session identity <name> [--json]` reads the local store only, as
`run_id` does, runs the same resolver without filling the cache, and prints
one JSON object whose keys are the attribute names: `koto.session.id`,
`koto.run.id`, `koto.run.template.name`, `koto.run.template.hash`. A key with
no value is left out. Without `--json` it prints the same pairs as
`key=value` lines. Exit 0 on success, even with keys left out; 2 for an
invalid or missing session, printed through `exit_with_error_code` like
`koto session hierarchy`; 3 when the store or header can't be read.

#### Alternatives Considered

**Walk to the root's header on every record.** No new state. Rejected: up to
64 header reads per record, and it fails silently once an intermediate
ancestor is gone.

**Record the root's template in the child's header at spawn.** Rejected: the
header is synced and re-serialized, and an older koto that rewrites it drops
the field for good, with no way to refill it.

**snake_case JSON keys.** Matches koto's other CLI output. Rejected: the
consumer would need a key map that can drift from the attribute names; the
dotted keys go straight through.

**Read through the cloud backend.** Rejected: `run_id` never reads from the
cloud, so the command could print values the stamps never carry.

## Decision Outcome

The four choices meet at `emit` and `flush`. Every record a hook builds goes
through `emit`, which stamps a clone with the run template from Decision 4,
drops it if it's a `context_reads`, and queues it with its session directory,
before writing the journal unchanged. `flush` is called at the four in-code
flush points and by the exit hook from Decision 3, which `main` registers
first so it runs after every other hook. A flush takes the queue, takes the
carry counts under the Decision 2 lock, calls the sink in chunks of 200
until the deadline, and on exit 3 or a kill adds the counts back and trips
the breaker. With no sink at the fixed path nothing is queued and nothing is
called, which is how release A stays inert on hosts until the rollout
installs a sink after 2026-11-09T02:00Z.

Release B is then mostly deletions. The journal half of `emit`, `rotate.rs`,
the readers directory, the `[run_journal]` keys (`enabled` included) and the
`context_reads` fold go; the queue, the sink call, the carry and the exit
hooks stay as they are.

## Solution Architecture

### Overview

koto keeps building records exactly as today. A new `src/telemetry/` module,
which is the moved `src/run_journal/` plus three files, queues a stamped copy
of each record and sends the queue to the sink at fixed points. The journal
write stays where it is in release A and can be turned off with
`run_journal.enabled = false`.

### Components

```
src/telemetry/          (was src/run_journal/)
  mod.rs                record builders and the five hooks, now calling emit
  emit.rs               emit(ctx, records, lock_wait): stamp, filter, enqueue, journal write
  queue.rs              the process queue, the breaker, flush()
  sink.rs               discovery and one sink call (spawn, stdin writer, deadline, group kill)
  carry.rs              telemetry.json, telemetry.lock and the orphan count
  sidecar.rs            run-journal.json, plus run_template_name and run_template_hash
  rotate.rs, context_reads.rs, driver.rs, fixture.rs, pairing.rs   (unchanged)
src/session/lineage.rs  (was src/run_journal/run_id.rs) run_id, child_lineage, run_template
src/names.rs            is_id, is_name, name_or_digest, digest
src/exit.rs             register, exit(code), crash(code)
src/cli/session.rs      koto session identity
src/config/             run_journal.enabled
```

The files on disk keep their names in release A: `_run_journal.jsonl`, the
`run-journal.json` sidecar and the `[run_journal]` config table. Only the
Rust module path changes.

### Key Interfaces

```rust
// src/telemetry/emit.rs
pub(crate) struct EmitCtx<'a> {
    pub journal_root: Option<&'a Path>,   // None: store has no journal, or run_journal.enabled = false
    pub session_dir: &'a Path,
    pub run_template: (Option<String>, Option<String>),  // (name, hash) from session::lineage
}
pub(crate) fn emit(ctx: &EmitCtx<'_>, records: &[Record], lock_wait: Duration);

// src/telemetry/queue.rs
pub fn flush();                    // the only entry point the flush sites call
struct Queued { line: String, session_dir: Arc<Path> }
enum Breaker { Open, StoppedCounting, StoppedSilent }

// src/telemetry/sink.rs
fn discover() -> Option<SinkPath>;  // cached in a OnceLock, first enqueue
fn send(path: &SinkPath, lines: &[&str], carry: u64, deadline: Instant) -> Outcome;
enum Outcome { Posted, Off, Dropped, PostedSomeInvalid, Usage, Killed, SpawnFailed }

// src/exit.rs
pub fn register(hook: impl FnOnce() + Send + 'static);
pub fn exit(code: i32) -> !;
pub fn crash(code: i32) -> !;     // no hooks: the simulated kill in cloud_takeover
```

**Discovery.** `discover` reads `FLEET_EVENT_SINK` and acts only on `off`.
It resolves `$HOME` (unset or relative means no sink), `lstat`s
`$HOME/.local/libexec/fleet/event-sink` and requires a regular file, not a
symlink, owned by `geteuid()`, with an execute bit and no group or other
write bit. It then canonicalizes the parent directory and checks every
component of the canonical path up to `/`: owned by the user or by root, no
group or other write bit. The sink is run by that canonical path. Since only
the user or root can write any directory on it, nobody else can swap a
component between the check and the spawn. The checks read metadata
through one function, `trusted(meta, uid)`, so the owner check can be
unit-tested without `chown`.

**The call.** `Command::new(path).args(["send", "--scope", "koto-journal"])`,
plus `--carry N` when N > 0, with `process_group(0)`, stdin piped, stdout
and stderr to null, and no environment change: koto passes nothing beyond
what it inherited and reads only the exit status. A writer thread writes the
chunk's lines and closes stdin; an `EPIPE` ends it quietly. The flushing
thread calls `wait_timeout(deadline - now)`. When that runs out it sends
`SIGKILL` to the group, reaps the child, and waits up to 100 ms for the
group to empty, reusing `wait_for_group_exit` from `src/action.rs` (made
`pub(crate)`). A spawn failure counts as a kill.

**The flush.** `flush()` takes the queue's items and releases the mutex
before anything slow, so a flush never blocks another thread's enqueue for
long. It returns at once when the queue is empty. With the breaker in
`StoppedCounting` it adds each item to its session's `dropped` and returns;
in `StoppedSilent` it discards them. Otherwise it sets the call deadline to
`start + 2.95 s`, takes the carry for the sessions in this flush, and sends
chunks of up to 200, the first with `--carry`. Exit 0 and 4 go on to the
next chunk; exit 1 or another code drops that chunk, counts it, and goes on;
exit 2 sets `StoppedSilent` and discards the rest uncounted; exit 3 or a kill
sets `StoppedCounting`, adds back the taken carry plus the failed chunk's
records, and counts every chunk not sent. A chunk the deadline doesn't reach
is dropped and counted the same way. The 50 ms between the call deadline and
3.0 s pays for that add-back, so the flush ends within 3.0 s of its start.

**Stamps.** On the queued clone only: `koto.run.template.name` and
`koto.run.template.hash` on every kind, each when it has a value, and on
each `check_rule` the `koto.check.outcome` of the `check_evaluated` record
before it in the same batch (`relayed` already writes them in that order).
Stamps go at the end of the field list. A stamped line over 4096 bytes is
counted as dropped instead of sent.

**`context_reads`.** `emit` skips any record whose kind is `context_reads`
before it clones anything, so no path, including anchor reads, reaches the
queue. A unit test hands `emit` a batch that holds one and asserts the queue
doesn't.

**`run_journal.enabled`.** A boolean in `RunJournalConfig`, default `true`,
read from the user layer only, like the other `[run_journal]` keys, and
resolved once per process with them. When it's `false`, `EmitCtx` carries no
journal root, so `emit` queues and returns without touching the journal.

### Flush points

Each flush point is one `telemetry::flush()` call at a fixed site. A flush
with an empty queue returns at once and makes no call, so a site that runs
often (a 100 ms wait slice) costs one mutex check when there's nothing to
send.

| # | Site | Where exactly |
|---|------|---------------|
| 1 | `src/action.rs`, `run_shell_command` | First statement, before `Command::new(SHELL)`. The action's 30 s timer (`started`, then `child.wait_timeout(timeout)`) starts after the spawn, so it opens after the flush returns. None of the four callers (`evaluate_command_gate` in `src/gate.rs`, `CheckEvaluator::evaluate`, and the default action and polling action in `src/cli/mod.rs`) starts a timer of its own before the call. |
| 2 | `src/cli/decider_port.rs`, the consultation | After the budget slot is taken, before `let started = Instant::now()`, so `latency_ms` still measures the provider alone. |
| 2 | `src/cli/check_evaluator.rs`, `consult` | Inside the retry loop, before `let started = Instant::now()`. A retry finds the queue empty. |
| 3 | `src/engine/poll.rs`, `settle` | Before `wait` is computed from `earliest`, so a slow flush shortens the sleep rather than pushing the next run back. |
| 3 | `src/cli/mod.rs`, the polling default action loop | Before the interval sleep after each run. |
| 3 | `src/cli/request.rs`, `sleep_until` | At the top of each loop iteration, before the `WAIT_SLICE` sleep. Every `koto request` wait goes through it. |
| 3 | `src/cli/check_evaluator.rs`, `lock_waiting` | After a failed try-lock, before the `LOCK_POLL` sleep. |
| 4 | `src/cli/mod.rs`, the terminal tick | After the `terminal` record and the parent notice, just before step 6 (`if !remove { return; }`), so it runs before `write_finished_record` and the removal, and the parent's records from the notice go in the same flush. |
| 5 | `src/exit.rs`, `exit(code)` | The telemetry hook `main` registers first, which runs last. |

No flush sits inside an action's 30 s timer. The only place one could is
`run_shell_command` between `spawn` and `wait_timeout`, and nothing is
called there. The sink is never run through `run_shell_command`, so flush
point 1 can't recurse.

### Data Flow

1. A hook builds its records, as today, and calls `emit` with its context.
2. `emit` runs discovery the first time. With no sink, or
   `FLEET_EVENT_SINK=off`, or the breaker in `StoppedSilent`, it skips
   queueing. Otherwise it drops `context_reads`, stamps a clone of each
   record and queues the lines with the session directory.
3. `emit` writes the unstamped records to the journal when `ctx` has a
   journal root.
4. At the next flush point, `flush` sends what's queued, and the exit hook
   sends whatever is left.
5. Drop counts go to `telemetry.json` in each record's session directory,
   and leave as `--carry` on the next successful call from any process.

### The stub-sink test harness

`tests/support/event_sink.rs` builds a scratch HOME that passes discovery
and installs a stub at the fixed path. The HOME is created under
`CARGO_TARGET_TMPDIR`, not the system temp directory: on Linux `/tmp` is
mode 1777, so a sink under it would fail the parent check and every test
would quietly test the no-sink path. The harness chmods its own directories
to 0700 and checks the full ancestor chain before a test starts, failing
with a clear message if the build directory's layout breaks discovery.

The stub is a shell script written by the harness. Each call appends one
JSON line to a log beside it: a high-resolution timestamp, its argv, and the
stdin lines it read. Its exit code, a sleep before exiting, and a check that
a named directory exists are set per test through files the stub reads, not
through koto's environment. Tests set `HOME` on the command they build with
`koto_command`, which already removes `KOTO_SESSION_*` and runs every test
process under its own home (`tests/support/test_home.rs`), so a test that
doesn't install a stub has none. Unit tests resolve the sink through the
test twin of the discovery cache and never look at the real HOME.

## Implementation Approach

Release A is seven pull requests, landed in this order. Each one passes the
full suite on its own, and none calls a sink until the fifth. Release A is
cut after the seventh, before 2026-11-09, and installed on hosts only by the
rollout after 2026-11-09T02:00Z.

### PR 1: Module moves and the journal golden test

A golden test first: one scripted run (init, a gate, a decider check, a
review record, a terminal) whose journal is compared byte for byte with a
checked-in fixture, each `at` value masked. Then the pure moves:
`src/run_journal/` to `src/telemetry/`, `run_id.rs` to
`src/session/lineage.rs`, and the four name functions to `src/names.rs`,
with every `crate::run_journal::` path and `tests/run_journal_doc_test.rs`'s
source directory updated. The golden test passes before and after.

### PR 2: The exit helper

`src/exit.rs`, `clippy.toml`'s ban, every exit site routed through
`exit` (or `crash` for the simulated kill), `main` ending in `exit(code)`,
and `EXIT_FLUSH` replaced by `register`. No telemetry yet; the cloud flush and
the scratch removal become the first two hooks.

### PR 3: `koto session identity` and the run template cache

The two sidecar fields, the resolver in `src/session/lineage.rs`,
`after_init` filling the cache, and the new subcommand with its docs and the
`koto-user` skill's mention of it. This is the piece tsukumogami/shirabe#1415
needs, so it lands early.

### PR 4: `run_journal.enabled`

The config key, its default, and the journal write skipped when it's
`false`. Needed by tsukumogami/koto#545.

### PR 5: The queue and the sink call

`emit`, the queue, discovery, the call with its deadline and group kill, the
breaker, the stamps, the `context_reads` exclusion, the telemetry exit hook
(flush point 5), the stub-sink harness, and a "Sent copy" section in
`docs/reference/run-journal.md` naming the three queue-only fields, which
keeps `tests/run_journal_doc_test.rs`'s two-way check honest.

### PR 6: The carry

`carry.rs`, the take and add-back around each call, the removal hook into
the orphan count, and `--carry` on the first chunk.

### PR 7: Flush points 1 to 4

The seven `flush()` calls in the table above, each with its ordering test.

### Release B (tsukumogami/koto#545)

Removes the journal half of `emit`, `rotate.rs`, the readers directory, the
`[run_journal]` keys (`enabled` included; a config that still has them loads
with at most a one-line notice) and the `context_reads` fold. The queue,
sink, carry and exit hooks are untouched, and the stub-sink tests from
release A run unchanged.

### Acceptance criteria and their tests

| Criterion (koto#544) | Test | PR |
|----------------------|------|----|
| No sink, or a sink failing the owner, mode, symlink or parent check: no call | `tests/event_sink_discovery_test.rs`: `no_sink_no_call`, `symlink_refused`, `group_writable_refused`, `parent_writable_refused`, `home_under_sticky_tmp_refused`; the owner check in `src/telemetry/sink.rs` unit tests through `trusted(meta, uid)` | 5 |
| `FLEET_EVENT_SINK=off` makes no call; another value changes nothing | `event_sink_discovery_test.rs`: `off_disables`, `other_values_ignored` | 5 |
| Flush point 1: the stub's call precedes the gate's start | `tests/event_sink_flush_points_test.rs`: `action_flush_precedes_gate_spawn` (the gate writes its own start time; the stub log's is earlier) | 7 |
| Flush point 2: records reach the stub before a provider call | `flush_points_test.rs`: `decider_flush_precedes_provider_call`, with a loopback stub provider logging its receive time | 7 |
| Flush point 3: polling gate, each `koto request` slice, lock poll | `flush_points_test.rs`: `poll_gate_flush_precedes_sleep`, `request_wait_flushes_before_slices`, `lock_poll_flush_precedes_sleep` | 7 |
| Flush point 4: after `terminal`, before removal | `flush_points_test.rs`: `terminal_flush_precedes_removal` (the stub checks the session directory still exists and that `terminal` is in its stdin) | 7 |
| Flush point 5, and no direct exit outside the helper | `tests/event_sink_exit_test.rs`: `exit_helper_flushes_queued_records`; clippy's `disallowed-methods` in CI; `src/exit.rs` unit test on hook order | 2, 5 |
| 450 records: 200, 200, 50; a deadline in the second call drops the third; the flush ends within 3.0 s | `src/telemetry/queue.rs` unit tests: `chunks_200_200_50`, `deadline_drops_unreached_chunk` | 5 |
| Exit 3: called once, later records counted, carry added back | `queue.rs`: `exit_3_trips_breaker_and_adds_back` | 5, 6 |
| A hung sink is killed with its group | `event_sink_exit_test.rs`: `hung_sink_killed_with_group` (the stub forks a child; nothing of the group is left) | 5 |
| Exit 2: called once, nothing counted | `queue.rs`: `exit_2_stops_without_counting` | 5 |
| Carry sent and zeroed; two processes never carry the same count | `tests/event_sink_carry_test.rs`: `carry_sent_and_zeroed`, `concurrent_processes_never_double_carry` | 6 |
| Same kinds and fields plus the stamps; journal byte-identical | `tests/event_sink_records_test.rs`: `sent_records_match_journal_plus_stamps`; the PR 1 golden test | 1, 5 |
| `context_reads` never enqueued | `src/telemetry/emit.rs`: `context_reads_never_enqueued` | 5 |
| `koto session identity` matches the header | `tests/session_identity_test.rs`: `root_identity_matches_header`, `child_carries_run_template`, `missing_session_exits_2` | 3 |
| `run_journal.enabled=false` stops the journal, sending goes on | `event_sink_records_test.rs`: `journal_disabled_still_sends` | 5 |
| Module moves change nothing | the PR 1 golden test, before and after | 1 |

The criterion that no backend product name, endpoint or private repository
name appears is checked by the reviewer against a list held outside this
repository, not by a test here.

## Consequences

### Positive

- One choke point for every record: a new hook gets journaling, sending, the
  stamps and the `context_reads` rule by calling `emit`, and can't skip any
  of them.
- Release A changes nothing on a host without a sink beyond one cached
  discovery check per process, so it can ship before the install date.
- Every exit now runs the same hooks, which also fixes the existing gap
  where a direct exit skipped `koto next`'s cloud flush.
- Release B is deletions, with the sending path and its tests untouched.

### Negative

- A flush can add up to 3.0 s to a command at each flush point that has
  records queued, including inside a polling window or a lock wait.
- Records queued before a panic, a `SIGKILL` or an unhandled signal are lost.
- Drop counts can undercount: a lock that times out on add-back, or counts
  added back in the instant between the orphan move and the directory's
  removal, are lost.
- The carry is unix-only.
- Each session directory gains `telemetry.json` and `telemetry.lock`, and
  each store gains `_telemetry_orphans.json` and its lock.

### Mitigations

- The breaker stops calls after the first failure in a process, so a broken
  sink costs one bounded flush per process, not one per flush point; an
  empty queue costs nothing.
- Panic coverage can be added later with `catch_unwind` around `run` in
  `main`, mapping a panic to `exit(101)`; it isn't needed for the contract.
- Undercounting is the failure the contract tolerates; carrying a count
  twice, which it doesn't, can't happen on unix.
- The new files are local to the host, mode 0600, and go with their session
  or store.
