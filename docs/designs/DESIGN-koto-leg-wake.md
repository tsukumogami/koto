---
schema: design/v1
status: Planned
upstream: docs/prds/PRD-koto-leg-wake.md
problem: |
  A coordinator session parked on a `request-leg` gate is never told its leg
  changed. The worker's terminal tick writes the result to the request log and
  exits; koto's wake pass runs only inside the waiting session's own tick, so
  it can fire only once that session is already awake; and the one waker in
  the crate logs and returns. The wake has to start where the leg changes and
  cross into another process that koto does not control.
decision: |
  The request store rings a per-session wake file, `~/.koto/wakes/<session>`,
  for the request's requester and coordinator of record every time its
  lock-held append records a result, records a refusal, abandons a leg, or
  closes the request. Ringing appends one opaque line with `O_APPEND` and no lock, and
  truncates the file in place first once it reaches 32 KiB. A harness
  subscribes by watching that file, or by running the new
  `koto request watch --session <id> --timeout-secs <n> [--since <cursor>]`,
  which polls it every 100 ms and exits when it changes. The wake-candidates
  pass in `koto next` rings the same file through a new `SignalWaker`, and
  `LoggingWaker` is deleted.
rationale: |
  Every leg change already funnels through the store's typed writes, so ringing
  there covers every route, runs in the process that made the change, and
  lands before that command returns. A plain file under `~/.koto/` is the one
  thing every harness can watch without koto naming any of them, and an
  append-only file that keeps one inode works for size pollers, native file
  watchers and `tail -F` alike. Polling one small file needs no dependency and
  no daemon. A configured hook, a waker in the waiting session's own tick,
  scanning request logs, and a socket or daemon were each weighed and lost on
  harness neutrality, on where the wake starts, or on footprint.
---

# DESIGN: koto-leg-wake

## Status

Planned

## Context and Problem Statement

The requirements are in the upstream PRD; this section states the technical
problem they leave.

A request leg changes in exactly one place. Every write that records a
result, records a refusal, abandons a leg, or closes a request goes through
the request store's lock-held append (`append_under_lock` in
`src/engine/request_store/mod.rs`). A worker's result reaches it through
`promote_leg_result` in `src/cli/mod.rs`, which runs during the worker's
terminal `koto next`, after the response has already been printed. Explicit
resolves, abandonments and closes reach it from `koto request` subcommands,
and refusals from `koto init --koto-leg`.

The session that waits on the leg is a different process, and usually a
different agent session in some harness. It reads the leg through a
`request-leg` gate when it ticks. Nothing on the write path above tells it to
tick. The existing wake machinery in `src/engine/wake.rs` looks like it should,
but it runs inside `handle_next` of the session that would be woken, walks
that session's own `ChildDispatched` events, and hands the wake to a
`SubstrateWaker` whose only implementation, `LoggingWaker`, prints a line.
Wiring a real waker there would still only fire once the coordinator is
already awake.

So the design has to settle two things: where the wake is produced, and what
carries it from that process to whatever harness is running the waiting
session, without koto knowing which harness that is. Everything is on one
machine: sessions live under `~/.koto/sessions/`, requests under
`~/.koto/requests/`, request writes rely on a host-local `flock`, and request
records do not replicate under the cloud backend. A wake across hosts is out
of scope by construction, not by omission: there is no shared store for one
host's write to reach another host's subscriber.

## Decision Drivers

- The wake must be produced by the process that changes the leg (PRD R1),
  and be observable before that process's command returns (R4).
- A harness must be able to subscribe with generic facilities, and koto must
  name no harness (R7).
- A wake carries no state and must be harmless when lost or repeated (R6).
  The waiting session always re-reads the request store.
- A wake must never fail the write that caused it (R5); the promotion path in
  particular can never fail a terminal tick.
- No new crate dependency and no long-lived koto process (R15); storage per
  session stays bounded (R14).
- The prior request-lifecycle design rejected filesystem-event watching inside
  koto (new dependency, blind under the cloud backend, confused by
  write-then-rename churn). A new design should not reintroduce it without
  answering those points.
- `RequesterWoken` keeps its fields and deduplication (R11), and `LoggingWaker`
  goes (R10).

## Considered Options

### Decision 1: Where the wake starts, and what carries it

The wake has to be produced somewhere a leg change is guaranteed to pass,
and then cross from that process into a harness koto knows nothing about.
The two halves are coupled: a carrier that needs a listener changes where the
wake can start, and a start point outside the store changes which routes are
covered. So they were decided together.

#### Chosen: the request store rings a per-session wake file; a watch command polls it

A leg's disposition comes from three event types only: a leg result, a leg
abandonment, and a request close. Every production route that writes one goes
through the store's lock-held append, `append_under_lock`. The append rings,
after it releases the lock, the wake file of each principal the request header
names. A promoted result, `koto request resolve`, `abandon`, `abandon-request`,
`close`, and the refusal `koto init --koto-leg` records all wake without any
of them having to remember to, and so does any future writer, because the
decision to ring is made where the event is written rather than in each typed
function. The typed functions pass whether their append can change a
disposition; the public `validate_and_append` derives it from the payload's
event type.

A successful return is the trigger, including a return the idempotency probe
answered without writing and an abandon of a leg that was already abandoned:
a retried resolve rings again, because the attempt it retries may have stopped
between its write and its ring. Promotion is the exception. It returns early
on an unlocked read when the leg already has a result, so a worker that
crashed between its write and its ring is not rung again by a later tick;
that costs latency, not correctness. `abandon-request` abandons each open leg
and then closes the request, so it rings each principal once per open leg plus
once for the close, all harmless.

The carrier is a file under `~/.koto/wakes/`, one per session name. A
harness can watch it with whatever it has (a file watcher, a poll, `tail -F`)
or run `koto request watch`, which polls the file and exits when it changes;
a harness that reacts to a background command finishing gets a wake as a
process exit. koto itself still does no filesystem-event watching: its watch
is a `stat`-and-read loop over one small file, which answers the objections
the request-lifecycle design raised. There is no dependency, the file is
appended in place so there is no rename churn, and the cloud backend is out of
scope because request records never leave the machine.

The file outlives the process that wrote it, so a subscriber that starts
late still finds the change against its cursor. That property is what makes
a lost wake cost only latency.

#### Alternatives Considered

**koto runs a user-configured hook on each leg change.** A `[wake] command`
in koto's config would be spawned with the session name. Rejected because the
configuration is where the harness gets named, contrary to R7; spawning from
a worker's terminal tick either blocks that tick on an external command or
leaves a child process behind (R15); and with nothing configured there is no
record for a subscriber that arrives later, so a lost wake is lost for good
rather than delayed.

**A real `SubstrateWaker` in the waiting session's own wake pass.** Keep the
wake where it is and give `LoggingWaker` a working replacement. Rejected
because the pass runs inside the waiting session's own `koto next`, so it
fires only after that session is already awake, which is the defect; it also
sees only `ChildDispatched` events, so explicit resolves, operator
abandonments and closes would never wake anyone. It survives only as a second
caller of the same ring (Decision 3).

**The subscriber polls the request logs.** `koto request watch` would scan
`list_requests --coordinator-of-record` and compare log sizes, with no new
write on the leg path. Rejected because nothing is produced by the process
that changes the leg (R1); telling a result from a progress append or a bind
(R3) means parsing logs on every poll, at a cost that grows with the number
of requests; it would make the request store's layout a public interface; and
a harness would have no single file to watch.

**A local socket, a daemon, or a harness message API.** A listener receives
the wake as a message. Rejected because it needs a process that outlives the
command (R15); a socket with no listener drops the wake, so a subscriber that
starts late misses it; and a harness message API names the harness (R7).

### Decision 2: How the wake file is written and read

With a file as the carrier, its representation decides who can watch it. It
has to work for a size poll, for a native file watcher registered on the
path, for `tail -F`, and for `koto request watch`; a cursor has to carry one
watch's position to the next; concurrent writers must not corrupt it; it must
stay under 64 KiB (R14); and it must carry no state (R6). The prior design's
point about write-then-rename is the constraint that removes most options: a
rename gives the path a new inode, and a watcher registered on the old one
goes quiet.

#### Chosen: an append-only log of opaque lines, truncated in place at a soft cap

The file is never renamed or replaced, so one inode backs the path for its
whole life. A ring opens it with `O_WRONLY | O_APPEND | O_CREAT | O_NOFOLLOW`
(mode 0600), and if `fstat` reports 32 KiB or more it calls `ftruncate(0)` on
that descriptor first. The open also carries `O_NONBLOCK`, and the descriptor
is refused unless `fstat` reports a regular file, so a FIFO or device planted
at the path can neither block a worker's terminal tick nor receive the write.
It then writes one line in a single `write`: an opaque token built from the
writer's wall-clock nanoseconds, its pid and a per-process counter, and a
newline, well under `PIPE_BUF`, so concurrent appends never interleave. The
counter matters because `abandon-request` rings one file several times from
one process in quick succession. There is no lock and no `fsync`: R4 asks for
the wake to be visible when the command returns, not durable across a crash,
and a lost wake is harmless. This is a stricter sibling of
`append_bounded_line` in `src/engine/jsonl_append.rs`: the same single
`O_APPEND` write, without its `fsync`, with symlink and file-type refusal and
the truncate check added.

The token exists only so that no two lines are byte-identical. It is not
state: nothing in koto reads it for a decision, and the docs call it opaque.

The cursor is `w1:<length>:<last token>`, read from the file's size and its
last complete line; an absent file reads as `w1:0:`. `koto request watch`
reports a wake when the cursor it reads differs from the one it started from.
Because the last token is unique per write, a file that was truncated and has
grown back to the same length still reads as changed. A direct poller that
compares only the size can miss a wake across a truncation, so the docs tell
pollers to compare size and modification time together; a native watcher or
`tail -F` sees every append as its own event.

A writer can overshoot the cap only if hundreds of concurrent writers all
read a size under 32 KiB before any of them truncates, which no request
produces; the file stays far below 64 KiB in practice and a small overshoot
would do no harm.

#### Alternatives Considered

**A small file replaced atomically with a fresh token.** Write a temp file
and rename it over the path, cursor equal to the token. Rejected because
every wake swaps the inode, the exact churn that silences a watcher
registered on the path, and a size poller sees no change when the token
length is fixed.

**An empty file whose modification time is touched.** Cursor equal to mtime.
Rejected because neither size nor content changes, so `tail -F` prints
nothing and most watchers see only an attribute event (R7); mtime resolution
is too coarse to tell two quick wakes apart on some filesystems, and a clock
change can move it backwards.

**A counter rewritten in place under a lock.** Cursor equal to the counter.
Rejected because the size changes only when the digit count does, so size
pollers and `tail -F` miss almost every wake; and it puts a lock, and a
lock-contention failure mode, into every wake, including the worker's terminal
tick.

### Decision 3: What happens to the existing wake pass and `LoggingWaker`

The wake-candidates pass in `handle_next` emits `RequesterWoken` for
dispatched children (`ChildDispatched` events on the coordinator's own log)
and hands delivery to a `SubstrateWaker` addressed to each child's
`requested_by`, which is not necessarily the session whose tick runs the pass.
The PRD deletes `LoggingWaker` and keeps `RequesterWoken`'s meaning. The two
mechanisms address different principals: the pass reads `requested_by` from
the dispatched child's session header, while a leg wake reads the request
log's header. Neither covers the other.

#### Chosen: a `SignalWaker` that rings the same file

A new `SignalWaker { koto_root }` implements `SubstrateWaker` by calling the
same ring function, and `handle_next` passes it to `wake_candidates_pass`.
`LoggingWaker` is deleted. The pass itself does not change: it emits
`RequesterWoken` with the same fields and the same `(child, epoch)`
deduplication, runs the same fsync sequence, and its age-and-activity recovery
rule still re-invokes the waker for a stale wake. Only the delivery behind the
event changes, from a stderr line to a ring, and the changelog says so under
Unreleased. The trait stays, because `docs/STABILITY.md` names it as the
surface an external substrate swaps in.

A session can receive a ring from both producers for related work. That costs
one extra tick that finds nothing new.

#### Alternatives Considered

**Drop the waker call from the pass.** The leg wake covers request legs, so
remove the pass's delivery. Rejected because `ChildDispatched` children are
tracked in a different store with different principals and would lose
delivery entirely, and the recovery rule would have nothing to retry.

**Keep `LoggingWaker` as a configurable fallback.** Rejected because R10
deletes it and names `koto request wait` as the fallback when nothing
subscribes; a stderr line nobody consumes is not worth a config key.

**Emit `RequesterWoken` from the leg-change path.** Merge the two into one
event. Rejected because legs have no child id or dispatch epoch for the
event's deduplication key (R11), it would make a worker write into its
coordinator's session log, and it would put an audit write on the promotion
path, which must never fail a terminal tick.

## Decision Outcome

**Chosen: the store rings an append-only per-session wake file; `koto request
watch` and any file watcher subscribe; the wake pass rings the same file.**

### Summary

When a leg changes, the process that changed it appends one line to
`~/.koto/wakes/<session>` for each session the request names as requester
or coordinator of record, once per distinct name. This happens inside the
request store, after the typed write returns and the lock is released, so
every route that can unblock a `request-leg` gate produces a wake, and the
line is in the file before the command that made the change returns. A
principal that fails koto's session-name grammar is skipped. Any failure to
ring becomes a warning on stderr; the write it followed has already
succeeded.

A harness subscribes in one of two ways. It can watch the file itself: a
native watcher or `tail -F` sees each append, and a poller compares size and
modification time. Or it can run `koto request watch --session <id>
--timeout-secs <n> [--since <cursor>]`, which polls the file every 100 ms and
exits zero with one JSON line carrying `cli_contract`, `session`, `woke`
and `cursor`,
when the file changes or the timeout passes. Passing the printed cursor as
`--since` to the next watch closes the gap between the two, so the loop "watch
in the background; when it exits, tick the coordinator; watch again from the
cursor" never misses a wake. A harness that reacts to a background command
finishing (a Claude Code session running the watch as a background task, for
instance) needs nothing else.

The wake means only "look again". The coordinator's next tick reads the leg
through its gate as it always has, so a lost wake costs latency and a
duplicate costs one tick that finds nothing new. Where nothing subscribes,
`koto request wait --timeout-secs` stays the documented way to wait. The
wake-candidates pass rings the same file through `SignalWaker`, and
`LoggingWaker` is gone.

### Rationale

The request store is the one place every leg change passes, and ringing
there is the only start point that satisfies R1 and R4 for every route
without each caller repeating it. A file is the one carrier that needs no
listener to exist at the moment of the change, which is what makes a late or
restarted subscriber safe, and it is the one thing every harness can observe
without koto naming it. Appending in place, rather than replacing, is what
lets native watchers and `tail -F` work at all, and the soft cap keeps the
file small without a lock. Reusing the file for the wake pass meets R10 and
R11 with a one-method adapter.

## Solution Architecture

### Components

- **`src/engine/wake_signal.rs` (new, built on every platform like the
  request store).** Owns the file layout and both ends:
  - `wake_path(koto_root, &ValidatedSessionId) -> PathBuf`:
    `<koto_root>/wakes/<session>`.
  - `ring(koto_root, principal: &str) -> Result<(), WakeSignalError>`:
    validates the principal with `ValidatedSessionId::new` (an invalid one is
    `Err(InvalidPrincipal)`), creates `wakes/` with mode 0700 if absent,
    refuses a symlinked directory, and appends one line as Decision 2
    describes. The directory and symlink helpers move out of the request
    store into a shared place rather than being copied a third time.
  - `ring_principals(koto_root, requested_by, coordinator_of_record)`: rings
    each distinct valid principal and prints a warning naming any principal it
    skipped or failed to ring. Never returns an error. On the refusal path this
    is the one stderr line `koto init --koto-leg` can add, and only on a
    failure; the refusal helper's doc comment says so.
  - `read_cursor(koto_root, &ValidatedSessionId) -> io::Result<WakeCursor>` and
    `WakeCursor` with `Display` / `FromStr` for the `w1:<len>:<token>` form.
    Reading takes the file's length and scans back from the end for the last
    complete line; an absent file is `w1:0:`.
- **`src/engine/request_store/mod.rs`.** `append_under_lock` takes a `ring`
  flag. When it is set and the call succeeds (written, idempotent, or a
  no-op), it rings the header's `requested_by` and `coordinator_of_record`
  after the lock guard is dropped; it has already read the view under the lock
  on every success path. `record_result`, `record_refusal`, `abandon_leg`,
  `abandon_leg_for_request` and `close_request` pass `true`; bind, attach,
  progress and create pass `false`. `validate_and_append` sets it when the
  payload is a leg result, a leg abandonment or a request close.
- **`src/engine/wake.rs` (unix-only, as today).** `LoggingWaker` is replaced
  by `SignalWaker { koto_root }`, which implements `SubstrateWaker` by calling
  `ring`. Nothing else in the pass changes. The doc link to `LoggingWaker` in
  `src/engine/respawn.rs` and the comment above the pass in `src/cli/mod.rs`
  are updated.
- **`src/cli/mod.rs`.** `handle_next` constructs `SignalWaker { koto_root }`
  instead of `LoggingWaker`.
- **`src/cli/request.rs`.** A `Watch { session, timeout_secs, since }`
  subcommand. It validates the session name and parses `--since` before any
  I/O: a bad session is `InvalidIdentifier` and a bad cursor
  `InvalidSubmission`, both exit 2 through the group's existing error
  envelope. It records the start cursor (or uses `--since`), then polls every
  100 ms with the same deadline, sleep-slice and signal handling
  `koto request wait` uses. A read failure other than a missing file is
  `PersistenceError` (exit 3). On a change it prints `woke: true`; at the
  deadline, `woke: false`; both exit 0. An interrupt exits in the transient
  class like `wait`. The response carries `cli_contract` like every other
  response in the group, and the group's contract minor is bumped so a caller
  that pins the new minor knows `watch` exists.

  The 100 ms interval is deliberately under `wait`'s one-second floor. That
  floor guards against re-reading and re-projecting a whole request log;
  a watch reads one file's size and at most its last line, which is cheap
  enough to do ten times a second and is what keeps the bound at 1 second.

### Data flow

```
worker session                     request store                  wakes/
koto next (terminal) ─► promote_leg_result ─► record_result ─┬─► append line to <coordinator>
                                                             └─► append line to <requester> (if different)

koto request resolve / abandon / abandon-request / close / koto init --koto-leg refusal
                                   ─► typed write ───────────────► same rings

coordinator's harness:   koto request watch --session <coordinator> --since <c>   (polls every 100 ms)
                            │ exits woke:true with new cursor
                            ▼
                         koto next <coordinator>  ─► request-leg gate reads the leg ─► advances
                         koto request watch --session <coordinator> --since <new cursor>
```

### Interfaces

- `koto request watch --session <id> --timeout-secs <n> [--since <cursor>]`,
  stdout: one JSON object `{"session": "<id>", "woke": <bool>, "cursor":
  "<opaque>", "cli_contract": {"major": 1, "minor": 2}}`. Exit 0 for both a wake and a
  timeout; 2 for a malformed session or cursor or a missing required flag; 3
  for an unreadable wake file; the transient class for an interrupt.
- The file `~/.koto/wakes/<session>`: append-only, one opaque line per
  wake, never renamed; may be truncated to empty. Its content is not an
  interface beyond "it changes on every wake".

### Bound

A ring happens before the command that made the leg change returns, so the
file has changed by the time that command exits (R4). `koto request watch`
polls every 100 ms, so a watch that was running before the change exits
within 100 ms plus one file read of that point; the documented bound is 1
second (R9), which leaves room for a loaded machine. The harness's own
reaction time after the watch exits is outside koto and outside the bound.

## Implementation Approach

1. **Engine signal module.** Add `wake_signal.rs` with the path, ring and
   cursor, plus unit tests: a ring appends one line, the
   cap truncates in place and keeps the inode, 10,000 rings stay under 64 KiB,
   cursors round-trip and detect a change across a truncation, an invalid
   principal is refused, a symlinked `wakes/` directory and a FIFO at the
   path are refused without blocking.
2. **Store rings and the waker swap.** Add the `ring` flag to
   `append_under_lock`, set it from the five typed writes and from
   `validate_and_append`'s payload type, replace `LoggingWaker` with
   `SignalWaker` in `wake.rs`, and pass it in `handle_next`. Unit tests in
   `request_store/tests.rs` for which writes ring and which do not, both
   principals, one ring when they match, and an unwritable `wakes/` that still
   lets the write succeed.
3. **`koto request watch`.** The subcommand, the contract minor bump, and
   unit tests for argument and cursor validation.
4. **End-to-end tests.** A new `tests/leg_wake_test.rs` driving the real
   binary in the style of `tests/request_leg_gate_test.rs`: the bound test
   with a watch running before the worker's terminal tick, the lost-wake and
   duplicate-wake tests, the cursor hand-off, the timeout, two concurrent
   watches, and a direct metadata poll with no koto command.
5. **Docs.** A leg-wake section in `docs/guides/cli-usage.md`, the wakes
   directory in `docs/workspace-layout.md`, the koto-user skill's guidance for
   agents running coordinators, and a CHANGELOG entry under Unreleased.

The steps build on each other in order, and the whole change is small enough
for one pull request.

## Security Considerations

The wake path is built from a principal string in the request header.
`koto request create` already checks `--requested-by` and
`--coordinator-of-record` against the session-name rules, so an invalid
principal can only arrive through the engine API or a hand-edited log. The
ring validates it again with `ValidatedSessionId::new` before any path is
joined, whose grammar rejects `/`, a leading `.`, NUL and names over 255
bytes, so a principal like `../../.ssh/x` never reaches the filesystem; it is
skipped with a warning. The file is opened with `O_NOFOLLOW` and the `wakes/`
directory is refused if it is a symlink, so a planted link cannot redirect a
ring into another file. The open also uses `O_NONBLOCK` and the descriptor is
refused unless it is a regular file, so a FIFO or device planted at the path
cannot block a worker's terminal tick, which would otherwise break R5. The
directory is created 0700 and files 0600, matching the request store.

A wake carries no state and nothing reads a line's content, so a forged wake
can do no more than make a session tick and find nothing new. Anyone who can
write to `~/.koto/wakes/` is already the same user and can tick the session
directly. The file cannot be grown without bound by a flood of rings: each
ring truncates it at 32 KiB.

Principals are not checked for existence, so a caller who creates many
requests naming many invented principals, and then closes them, leaves one
small file per name under `wakes/`. That is the same exposure the request
store already has, since each of those requests leaves its own directory, and
it needs the same user's access to create. On a case-insensitive filesystem
two session names differing only in case share one wake file; the effect is
an extra harmless wake, the same collision their session directories already
have.

`koto request watch` only reads one file and prints its own cursor; it
writes nothing and runs no command. No new process outlives the command that
rang, and nothing crosses the machine boundary.

## Consequences

### Positive

- A coordinator parked on a leg is woken from the worker's own terminal tick,
  and from every other route that changes a leg, with a documented bound.
- Any harness can subscribe with tools it already has, and koto's code and
  config name none of them.
- A lost wake costs latency and a duplicate costs one idle tick; correctness
  never depends on the wake.
- `LoggingWaker`'s misleading "not yet wired" line is gone, and the existing
  wake pass finally delivers.

### Negative

- A new directory under `~/.koto/` accumulates one small file per session
  name ever woken. Each is at most about 32 KiB.
- A subscriber that polls size alone can miss a wake across a truncation.
- The existing recovery rule in the wake pass can re-ring an idle requester
  on every coordinator tick once its timeout passes, because a blocked tick
  does not touch the requester's log. That was already its behaviour against
  `LoggingWaker`; it is now visible as extra wakes.
- `koto request watch` is a new command to keep stable.

### Mitigations

- The files are tiny and bounded one per principal name; a sweep can be added
  to workspace pruning if they ever matter.
- The docs tell pollers to compare size and modification time, and point
  native watchers and `tail -F` at the file directly.
- Extra wakes from the recovery rule are harmless by design; tightening the
  rule to fire once per timeout window is left as follow-up work rather than
  changed here, since the PRD keeps the pass's semantics.
- The watch shares `koto request wait`'s envelope, exit classes and polling
  code, so it adds little new surface.
