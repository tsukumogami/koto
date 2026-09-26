---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
upstream: docs/designs/DESIGN-koto-leg-wake.md
milestone: "Leg wake"
issue_count: 5
---

# PLAN: koto-leg-wake

## Status

Active

## Scope Summary

Build the leg wake the DESIGN settles: the request store rings a per-session
wake file on every leg result, abandonment and close; `koto request watch`
and direct file watchers subscribe; the wake-candidates pass rings the same
file and `LoggingWaker` is deleted. Everything lands in one pull request.

## Decomposition Strategy

**Horizontal decomposition.** The DESIGN's implementation approach is five
layers with stable interfaces between them: the signal module, the store's
rings and the waker swap, the watch command, the end-to-end tests, and the
docs. The signal module is a prerequisite for every other piece and the
end-to-end tests need both the rings and the watch, so there is no thinner
vertical slice worth building first. The work does not split: koto declares
no delivery preference, nothing forces an order across pull requests, and a
watch with no rings, or rings with no documented way to subscribe, is not
useful on its own.

## Issue Outlines

### Issue 1: feat(engine): add the per-session wake signal

**Complexity**: testable

**Goal**: Add `src/engine/wake_signal.rs`, built on every platform: the wake
path `<koto_root>/wakes/<session>`, `ring`, `ring_principals`, and the
`w1:<len>:<token>` cursor with `read_cursor`, as the DESIGN's Decision 2 and
Solution Architecture describe. Move the request store's directory-0700 and
symlink-refusal helpers somewhere both can share rather than copying them.

**Acceptance Criteria**:
- [ ] A ring appends exactly one line of the form
  `<nanoseconds>.<pid>.<counter>`: the pid field equals the writing process's
  pid, and two rings from one process within the same nanosecond reading
  (forced through a test seam) still differ in the counter field.
- [ ] The file is opened with `O_APPEND | O_CREAT | O_NOFOLLOW | O_NONBLOCK`,
  mode 0600 in a `wakes/` directory created 0700, and a non-regular file (a
  FIFO) at the path is refused without blocking; a symlinked `wakes/`
  directory is refused.
- [ ] Once the file is 32 KiB or more, a ring truncates it in place before
  appending; the inode is unchanged, and after 10,000 rings the file is at
  most 64 KiB.
- [ ] `read_cursor` on an absent file is `w1:0:`; a cursor round-trips
  through `Display` and `FromStr`; a malformed cursor fails to parse; the
  cursor differs after every ring, including a ring that follows a truncation
  and leaves the length unchanged.
- [ ] `ring` refuses a principal that fails `ValidatedSessionId::new`;
  `ring_principals` rings a shared name once, rings both of two distinct
  names, skips an invalid one with a warning and still rings the other, and
  never returns an error.

**Dependencies**: None

### Issue 2: feat(engine): ring the wake from the request store and the wake pass

**Complexity**: critical

**Goal**: Give `append_under_lock` a `ring` flag and ring the header's
requester and coordinator of record after the lock is released on every
successful return; set it from `record_result`, `record_refusal`,
`abandon_leg`, `abandon_leg_for_request` and `close_request`, and from
`validate_and_append` when the payload is a leg result, leg abandonment or
request close. Replace `LoggingWaker` with `SignalWaker { koto_root }` in
`src/engine/wake.rs`, pass it from `handle_next`, and update the stale doc
link in `src/engine/respawn.rs`, the comment above the pass, and the refusal
helper's doc comment.

**Acceptance Criteria**:
- [ ] Each of a promoted result, an explicit resolve, a refusal, a leg
  abandonment, a request abandonment, and a close changes the addressed
  session's wake cursor before the call returns.
- [ ] Creating a request, binding a leg, attaching a leg, and appending
  progress leave the cursor unchanged.
- [ ] Distinct requester and coordinator of record both ring and a third
  session's cursor is unchanged; matching names ring once per write.
- [ ] A retried explicit resolve that the idempotency probe answers rings
  again; abandoning an already-abandoned leg rings.
- [ ] A request whose coordinator of record is invalid (built through the
  engine API) still resolves and still wakes a valid requester, and the
  mirror case wakes the coordinator of record.
- [ ] With the `wakes/` directory unwritable, a resolve still succeeds,
  records the result, and prints a warning to stderr.
- [ ] The wake pass's `SignalWaker` rings the requester when it emits
  `RequesterWoken`; the existing wake-pass tests pass unchanged; `LoggingWaker`
  no longer exists in the crate, and `cargo doc --no-deps` reports no broken
  intra-doc link to it.

**Dependencies**: Issue 1

### Issue 3: feat(cli): add koto request watch

**Complexity**: testable

**Goal**: Add `koto request watch --session <id> --timeout-secs <n> [--since
<cursor>]` to `src/cli/request.rs` as the DESIGN's Interfaces section
specifies, and bump the request group's contract minor.

**Acceptance Criteria**:
- [ ] A missing `--session` or `--timeout-secs` is a usage error; an invalid
  session is `InvalidIdentifier` and an unparseable `--since` is
  `InvalidSubmission`, both exit 2 before any I/O.
- [ ] With no wake, the watch exits 0 at its timeout printing
  `{"cli_contract", "session", "woke": false, "cursor"}`.
- [ ] A ring during the watch makes it exit 0 with `woke: true` and the new
  cursor, within 1 second of the ring.
- [ ] Passing an earlier cursor as `--since` returns at once when the file
  changed since.
- [ ] After 10,000 rings have truncated the file at least once, a running
  watch still exits `woke: true` on the next ring.
- [ ] An unreadable wake file (for example a directory at the path) makes the
  watch exit 3 with `PersistenceError`.
- [ ] `CLI_CONTRACT_MINOR` is bumped and every request response reports the
  new minor.

**Dependencies**: Issue 1

### Issue 4: test: drive the leg wake end to end

**Complexity**: testable

**Goal**: Add `tests/leg_wake_test.rs`, driving the real binary in the style
of `tests/request_leg_gate_test.rs`, covering the PRD's end-to-end
acceptance criteria.

**Acceptance Criteria**:
- [ ] With a coordinator blocked on a `request-leg` gate and a watch started
  for it, an attached worker's terminal tick makes the watch print
  `woke: true` within 1 second of that tick returning, the coordinator's next
  tick passes the gate, and no koto process started by the worker's tick is
  still running.
- [ ] Lost wake: with no watch running, the coordinator's next tick after the
  worker finishes still passes the gate.
- [ ] Duplicate wake: a second ring and a second coordinator tick leave the
  coordinator's event log with the same transitions as one tick would, and
  the second tick reports the same state as the first.
- [ ] Cursor hand-off: a watch given the first watch's cursor returns
  `woke: true` for a wake delivered between the two.
- [ ] Two concurrent watches on one session both return `woke: true` for one
  wake.
- [ ] A resolve naming a session that was never initialised creates its wake
  file, and a watch started beforehand sees it.
- [ ] A test polling the wake file's size and modification time, with no koto
  command, detects a wake.

**Dependencies**: Issue 2, Issue 3

### Issue 5: docs: document the leg wake and its fallback

**Complexity**: simple

**Goal**: Document the wake for users and agents: a leg-wake section in
`docs/guides/cli-usage.md`, the `wakes/` directory in
`docs/workspace-layout.md`, the koto-user skill's coordinator guidance, and a
CHANGELOG entry under Unreleased.

**Acceptance Criteria**:
- [ ] `docs/guides/cli-usage.md` covers which writes ring, whom they address,
  the file path, both ways to subscribe (watching the file, comparing size
  and modification time when polling; and `koto request watch` with the
  cursor loop), the 1-second bound, that a wake carries no state and a lost
  or duplicate one is harmless, the local-only limit, and
  `koto request wait --timeout-secs` as the fallback when nothing subscribes.
- [ ] The koto-user skill tells an agent running a coordinator how to
  subscribe and points at the guide.
- [ ] `CHANGELOG.md` has an entry under Unreleased for the wake,
  `koto request watch`, and `RequesterWoken` now being delivered through the
  wake file; the stale 0.13.0 block there is left alone.
- [ ] `cargo test --test doc_names` passes.
- [ ] `Cargo.toml` and `Cargo.lock` gain no new dependency across the whole
  pull request.

**Dependencies**: Issue 2, Issue 3

## Implementation Sequence

**Critical path:** Issue 1 -> Issue 2 -> Issue 4 (Issue 3 runs alongside
Issue 2 and also feeds Issue 4).

**Recommended order:**
1. Issue 1: the signal module every other piece calls.
2. Issue 2: the rings and the waker swap, the part that changes existing
   behaviour.
3. Issue 3: the watch command, which needs only Issue 1.
4. Issue 4: the end-to-end tests, once rings and watch both exist.
5. Issue 5: the docs, once the rings and the watch's interface are fixed.

**Parallelization:** After Issue 1, Issues 2 and 3 are independent.
