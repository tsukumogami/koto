---
schema: prd/v1
absorbed: docs/briefs/BRIEF-koto-leg-wake.md
status: Done
problem: |
  A coordinator session parked on a `request-leg` gate is never told that the
  leg changed. The worker's terminal tick records its result and exits, koto's
  wake pass only runs inside the waiting session's own tick, and the only
  waker logs and returns. Coordinators sit idle until an unrelated cue, or
  hold a turn open polling one leg with `koto request wait`.
goals: |
  When a leg resolves by any route or is abandoned, the sessions the request
  names are woken within a documented bound, through a signal any harness can
  subscribe to without koto knowing which harness it is. Lost and duplicate
  wakes cost nothing, and `koto request wait` stays as the documented fallback.
source_issue: 250
---

# PRD: koto-leg-wake

## Status

Done

Absorbed [BRIEF](docs/briefs/BRIEF-koto-leg-wake.md); carried in Absorbed Brief.

## Absorbed Brief

**Why this exists.** koto's request legs let one session wait on another,
and a coordinator workflow that dispatches several workers and waits on their
legs is being built on them. A coordinator parked on a `request-leg` gate is
never told the leg changed, so it waits on an unrelated cue or holds a turn
open in `koto request wait`.

**The outcome.** A coordinator author parks a session on a leg and stops
thinking about it. When any leg it waits on resolves by any route, or is
abandoned, the harness running the coordinator hears within a documented,
short time and ticks it, with no koto configuration naming the harness. A
missed signal is caught on the next tick and a repeated one finds nothing new.
With no harness listening, `koto request wait` stays the way to wait.

**The journeys.** A coordinator is woken by its worker finishing; a
coordinator is woken by an abandonment no worker caused; a harness
integrator wires the signal into their harness once, from koto's docs alone;
a script with no subscriber keeps waiting with a timeout as it does today.

**The boundary.** In: wakes on every route that resolves or abandons a leg,
addressed to the request's named sessions on the same machine, a documented
subscription, harmless lost and duplicate wakes, and retiring the
logging-only waker. Out: cross-host wakes, harness-specific code, the
consuming workflow, state in the wake, a daemon, and the cloud backend.

## Problem Statement

A koto request leg is how one session waits on another. A coordinator
creates a request, a worker session attaches to one of its legs, and the
worker's terminal tick promotes its result onto the leg. The coordinator's
template reads the leg through a `request-leg` gate and stays blocked while
the leg is open.

Nothing tells the coordinator that the leg changed. The worker's tick
writes to the request log and exits. The coordinator runs in another
process, usually another agent session, and learns about the change only
when it next runs `koto next`, which its agent has no reason to do. koto's
existing wake pass cannot help: it runs inside the waiting session's own
tick, so it fires only after that session is already awake, and the one
waker the crate ships prints "not yet wired" and returns.

The consequence is that every coordinator built on request legs either
waits an unbounded time for an unrelated cue, or holds its agent's turn open
with `koto request wait --leg <name> --timeout-secs <N>`, which watches one
leg of one request and keeps the agent from reacting to anything else. A
coordinator workflow that dispatches several workers and also listens for
harness messages cannot be built well on either.

## Goals

- A session waiting on a leg hears that the leg changed without ticking
  first, within a bound koto documents.
- Any harness can subscribe to that signal using what it already has for
  noticing events, and koto names no harness.
- The signal carries no state, so losing it or receiving it twice never
  changes what the waiting session does; it only changes when.
- Environments with no subscriber keep working exactly as they do today,
  with `koto request wait` documented as the way to wait there.

## User Stories

- As a coordinator workflow author, I want my session parked on a worker's
  leg to be ticked soon after the worker finishes, so that my workflow moves
  on without me or an unrelated message nudging it.
- As a coordinator workflow author, I want a leg abandoned by an operator or
  by a newer run to wake my session the same way a result does, so that it
  routes on the abandonment instead of waiting for a worker that will never
  answer.
- As a harness integrator, I want one documented signal per waiting session
  and a documented way to watch it, so that I can wire wakes into my
  harness's own event loop without koto knowing my harness exists.
- As a coordinator author with no harness listening, I want the existing
  timeout wait to remain the documented way to wait, so that scripts that work
  today keep working.

## Requirements

Two terms from the request header recur below. The **requester** is the
principal recorded as `requested_by` when a request is created; the
**coordinator of record** is the principal recorded as
`coordinator_of_record`, answerable for the request and possibly outliving
the session that created it. Both are free strings chosen by whoever runs
`koto request create`. A principal is a **valid session identifier** when it
passes the same session-name grammar `koto init` enforces.

### Functional

- **R1. A wake originates where the leg changes.** Every write that records
  a leg's result (a promoted worker result, an explicit resolve, or a refusal
  koto records), abandons a leg (one leg, or every leg of an abandoned
  request), or closes a request delivers a wake. The wake is produced by the
  process making that write, not by the waiting session.
- **R2. The wake addresses the request's named principals.** A wake is
  delivered to the coordinator of record and to the requester. When both
  name the same session, that session's signal changes once for the write,
  not twice. A principal that is not a valid session identifier is skipped
  and the other principal is still woken. A wake is delivered whether or not
  a session by that name currently exists, and it never touches the signal
  of a session the request does not name.
- **R3. No wake for changes a waiter cannot act on.** Creating a request,
  binding or attaching a leg, and appending progress deliver no wake.
- **R4. The wake is present before the write's caller regains control.**
  When the command that made the leg change returns (for a promoted result,
  the worker's terminal `koto next`), the addressed signal has already
  changed. The wake is produced after the leg change is durable, so a
  subscriber that sees the wake and then reads the request always sees the
  change.
- **R5. A wake never fails the write that caused it.** If a wake cannot be
  delivered, the leg change still succeeds, the command exits with the code
  it would have had, and a warning naming the principal is printed to stderr.
- **R6. A wake carries no state.** Its only meaning is "look again". The
  leg's result, disposition, and identity are read from the request store, not
  from the wake, and nothing in koto reads a wake's content as input to a
  workflow decision.
- **R7. Harness-neutral subscription, two supported ways.** Each session's
  wake signal is a file at a documented, stable path under koto's home
  directory, and every wake changes that file's size or content. Watching that
  file directly (by polling or with the harness's own file watcher) is a
  supported, documented way to subscribe. The watch command in R8 is the
  other. koto's configuration and code name no harness.
- **R8. `koto request watch`.** koto provides
  `koto request watch --session <id> --timeout-secs <n> [--since <cursor>]`.
  `--session` and `--timeout-secs` are required, as they are for
  `koto request wait`; omitting either is a usage error. The command blocks
  until the named session's signal changes relative to the cursor (or, with
  no `--since`, relative to its state when the command started), or until the
  timeout passes, and exits zero in both cases. It prints one JSON object
  carrying `session`, `woke` (true when a wake arrived, false at timeout), and
  `cursor`, an opaque string. Passing that cursor as `--since` to the next
  invocation makes a wake delivered between the two invocations return at
  once. A cursor that koto cannot parse is a usage error. Any number of
  watches may run for one session at once, and each sees every wake.
- **R9. Wake bound.** A watch started before the leg change returns within 1
  second of the command that made the change returning, as measured by the
  crate's own tests in CI.
- **R10. The logging-only waker is removed.** The wake-candidates pass in
  `koto next` delivers its wake through the same signal as R1. `LoggingWaker`
  is deleted from the crate; the fallback when nothing subscribes is
  `koto request wait`, not a logging waker.
- **R11. `RequesterWoken` keeps its meaning.** The audit event is still
  emitted by the wake-candidates pass with the same fields and the same
  deduplication; only the delivery behind it changes, and that change is
  recorded in the changelog under Unreleased.
- **R12. Documentation.** The CLI guide documents the wake: which writes
  produce it, whom it addresses, the signal's path, both ways to subscribe,
  the bound, that a wake carries no state and a lost or duplicate one is
  harmless, the local-only limit, and `koto request wait --timeout-secs` as
  the fallback when nothing subscribes. The koto-user skill tells agents
  running coordinators how to use it.

### Non-functional

- **R13. Local only.** Wakes are delivered on the machine that made the leg
  change. A coordinator on another host is never woken, and the docs say so.
- **R14. Bounded footprint.** A session's signal file never exceeds 64 KiB,
  however many wakes it receives without being read.
- **R15. No new long-lived process and no new crate dependency** for
  delivering or watching wakes. No koto process outlives the command that
  delivered a wake.

## Acceptance Criteria

- [ ] End to end through the real binary: with a coordinator blocked on a
  `request-leg` gate and `koto request watch` running for it, an attached
  worker reaches its terminal state; the watch prints `woke: true` within 1
  second of the worker's terminal `koto next` returning, and the
  coordinator's next tick passes the gate.
- [ ] For each of promoted result, explicit resolve, refusal, leg
  abandonment, request abandonment, and close: when the command returns, the
  addressed session's signal file differs from its state before the command.
- [ ] Creating a request, binding a leg, attaching a leg, and appending
  progress each leave the addressed session's signal file unchanged.
- [ ] A request whose requester and coordinator of record differ changes
  both sessions' signal files on a resolve; a third session's signal file is
  unchanged.
- [ ] A request whose requester and coordinator of record are the same
  session produces exactly one change (one appended wake) per resolve.
- [ ] A request whose coordinator of record is not a valid session
  identifier still resolves successfully and still wakes a valid requester,
  and the mirror case (invalid requester, valid coordinator of record) wakes
  the coordinator of record.
- [ ] A resolve on a request naming a session that has never been
  initialised creates that name's signal file, so a watch started before
  the session exists still sees the wake.
- [ ] With the wake directory made unwritable, a resolve still exits zero,
  records the result, and prints a warning to stderr.
- [ ] Lost wake: with no watch running when the worker finishes, the
  coordinator's next tick still passes the gate.
- [ ] Duplicate wake: after the worker finishes, delivering a second wake and
  ticking the coordinator twice leaves its event log with the same
  transitions as a single tick, and the second tick reports the same state.
- [ ] A watch given the cursor from an earlier watch returns `woke: true`
  within 1 second when a wake arrived between the two invocations.
- [ ] A watch with no wake exits zero at its timeout and prints
  `woke: false`; omitting `--timeout-secs` or `--session` exits with a usage
  error, and so does an unparseable `--since`.
- [ ] Two watches on the same session both return `woke: true` for one wake.
- [ ] After 10,000 wakes with no reader, the session's signal file is at
  most 64 KiB, and a watch still detects the next wake.
- [ ] A test polling the signal file's metadata, without calling any koto
  command, detects a wake.
- [ ] The wake-candidates pass rings the requester's signal when it emits
  `RequesterWoken`, and the existing wake-pass tests for the event's fields
  and deduplication pass unchanged. `LoggingWaker` no longer exists in the
  crate.
- [ ] `CHANGELOG.md` has an entry under Unreleased describing the wake,
  `koto request watch`, and the change to `RequesterWoken`'s delivery.
- [ ] `docs/guides/cli-usage.md` has a section on leg wakes covering each
  item R12 lists, the koto-user skill references it, and
  `cargo test --test doc_names` passes.
- [ ] `Cargo.toml` gains no dependency, and in the end-to-end test no koto
  process started by the worker's tick is still running once that tick has
  returned.
- [ ] `cargo test`, `cargo clippy`, and `cargo fmt --check` pass, and so does
  every check CI runs on the pull request.

## Out of Scope

- Wakes across hosts. The session and request stores live under the user's
  home directory on one machine, and request records do not replicate under
  the cloud backend.
- Calling into a specific harness, message bus, or notification service.
- Changes to the coordinator workflow that consumes the wake, which lives in
  another project and adapts after this lands.
- Carrying results or any other state in the wake.
- A koto daemon or other long-running service.
- The cloud session backend.

## Decisions and Trade-offs

- **Both principals are woken.** The brief left open which of the request's
  named principals a wake addresses when they differ. The coordinator of
  record is the session answerable for the request and the requester is who
  asked; a `request-leg` gate can sit in either. Waking only the coordinator
  of record was the alternative. Waking both costs at most one extra harmless
  wake and never strands a waiter.
- **Close wakes; progress and bind do not.** Waking on every append was the
  alternative. Progress appends can be frequent and a gate never unblocks on
  them, so they would only produce noise. Close is included because it is the
  last change a request sees and a wake there is cheap.
- **The bound is stated in two parts.** A single end-to-end number from worker
  finish to coordinator tick was the alternative, but the harness's reaction
  time is outside koto's control. koto commits to the wake being durable when
  the resolving command returns and to its own watch noticing within 1 second.
