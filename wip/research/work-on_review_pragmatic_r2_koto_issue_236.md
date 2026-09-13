# Pragmatic review, round 2 -- koto#236 state-log header guard

Scope: `git diff 5b8cfbf..HEAD -- . ':!wip'`, with the round-2 delta (commit
9f08117) as the focus. Read-only review; nothing outside this file was touched.

Verdict: **0 blocking, 4 advisory.** The round-2 delta is smaller and more
honest than what it replaced. Nothing in it is over-built, and the one piece of
new redundancy is a duplicated file read that costs a few microseconds on logs
that hold tens of lines.

Verification run: `cargo test --lib persistence` -- 57 passed, 0 failed.

## The specific question: is `next_seq_after_header` called twice?

No. It is called exactly once on the idempotent path, at
`src/engine/persistence.rs:337`. What happens twice is the **file read**: line
337 reads the whole log inside `next_seq_after_header`
(`src/engine/persistence.rs:529`), and line 343 reads the whole log again for
the hash scan.

Does it matter?

- **Correctness: no.** The `flock` at `src/engine/persistence.rs:331` is held
  across both reads, and every writer that reaches a request log goes through
  this path (also under `request.lock`). The two reads cannot disagree in a way
  that matters, so the `next_seq` taken from read 1 is still right when the
  append at line 388 runs.
- **Cost: a genuine but tiny regression on one path.** Before the reorder, an
  idempotent *hit* short-circuited after a single read; `next_seq_after_header`
  ran only on the miss path. Now a hit pays two full reads. The miss path is
  unchanged (it read twice before too). Request logs are small, so this is
  noise, not a problem.

So the reorder bought a real correctness property (a headerless log is refused
even when the scan would have hit) for one extra read on the hit path. That is
a good trade, and the deleted `path.exists()` plus the un-nested loop body make
the function shorter than it was. No complaint about the change itself.

### Advisory 1 -- the second read is removable in a few lines

`src/engine/persistence.rs:337` and `src/engine/persistence.rs:343` read the
same bytes under the same lock. If you want it gone, the smallest version is a
private helper next to `next_seq_after_header` that returns both halves of the
one read it already does:

```rust
fn read_log_after_header<H: LogHeader>(path: &Path) -> anyhow::Result<(String, u64)>
```

`next_seq_after_header` then becomes a one-line wrapper for the two callers
that only want the seq (`append_event_in` at :160), and the idempotent path
drops its own `read_to_string`. This is worth doing only if you were going to
touch the function anyway -- it is not worth a round-3 commit on its own, and
the follow-up note already flags this area for the write-lock design.

### Advisory 2 -- the reorder has no test

The behavior the round-2 commit added is "a headerless log is refused *even
when the hash scan would have found a match*". No test exercises it. The
closest, `append_event_idempotent_refuses_a_missing_log_and_creates_nothing`
(`src/engine/persistence.rs:1899`), passes with either ordering, because a
missing log fails earlier still, at the non-creating open in
`acquire_state_flock` (`src/engine/persistence.rs:414`). Nothing in
`tests/state_log_integrity_test.rs` covers the idempotent path at all (no
`idempot` match in that file), and `tests/idempotency.rs` always writes a
header first (`tests/idempotency.rs:56`).

Concrete fix, one unit test: write a headerless log whose single line is an
event carrying `idempotency_hash: "h"`, call `append_event_idempotent(..,
Some("h"))`, assert it errors with `state log has no header` and that the file
is byte-for-byte unchanged. Without it, a future reader who sees the same file
read twice can "simplify" the check back below the scan and every test stays
green.

## Are the new comments earning their space?

Mostly yes. Checked each one:

- `src/cli/mod.rs:1419-1433` (8 lines over a 10-line block). **Earns it.** The
  exit-code half is not derivable from the code, and it is accurate: I verified
  `EXIT_CALLER_ERROR = 2` and `EXIT_INFRASTRUCTURE = 3` (`src/cli/mod.rs:75`,
  `src/cli/mod.rs:80`), and the handler's error arm at `src/cli/mod.rs:1442`
  does exit 3 for a header that won't parse. Keep as is.
- `src/engine/request_store/mod.rs:1256-1257`. **Earns it.** Why this call site
  passes `RequestHeader` rather than the session header is exactly what a
  reader stumbles on at a turbofish.
- `src/engine/persistence.rs:333-337`. **Earns it.** States the ordering
  constraint and what breaks without it. The trailing "Line 1 is the header,
  checked above, so events start at line 2" (:341) restates `.skip(1)`, but it
  is the sentence that ties the skip to the check above, so it survives.
- `src/session/local.rs:281-283` and `src/session/mod.rs:255-259`. **Earn it,
  and they were previously wrong** -- the old `local.rs` comment claimed
  `append_header`/`append_event` create state files, which the guard made
  false. Fixing them was the right call.

### Advisory 3 -- the same rule is now written out about ten times

"An append never creates a log" is stated at `src/engine/persistence.rs:139`,
:149, :275, :307, :333, :411, :515, plus `src/session/mod.rs:255`,
`src/session/local.rs:281`, `src/cli/mod.rs:1419` and
`src/cli/context.rs:23`/:147. Each site is defensible alone, and several are
public API surfaces that genuinely need it. But round 2 existed partly to fix
three of these that had gone stale, which is the maintenance cost showing up
already. Suggestion, at your discretion: keep the full argument on
`append_event` (:139) and `next_seq_after_header` (:515), and let the internal
ones (:307, :411) shrink to a pointer. Not worth a commit on its own.

### Advisory 4 -- `append_event_idempotent` has no production callers left

Since the request store switched to `append_event_idempotent_in::<RequestHeader>`
(`src/engine/request_store/mod.rs:1258`), the `StateFileHeader` wrapper at
`src/engine/persistence.rs:297` is called only from tests
(`tests/idempotency.rs`, and `src/engine/persistence.rs:1903`). It is a
four-line wrapper carrying the module's main doc comment, so keeping it as the
documented session-log entry point is a defensible call -- but if no session-log
caller is coming, having the tests call `_in::<StateFileHeader>` and deleting
the wrapper removes a public function nothing ships. This predates the round-2
delta; noting it because it is the one thing in the diff that is dead in
production.

## On the round-2 declines

All five declines are reasonable, and I am not re-raising any of them.

- The dispatch `exists` check versus the handler's `read_header` do pick
  different exit codes and guard different windows (`src/cli/mod.rs:1430` vs
  `src/cli/context.rs:23`). Merging them would have to pick one exit code and
  would lose information. Correct decline. The follow-up note already records
  that a lock held across both makes one check enough later
  (`wip/followup_state-log-write-lock.md:89`).
- `append_event_in` staying public: needed by the generic wrapper's callers and
  symmetric with `append_event_idempotent_in`. Fine.
- The duplicated batch-parent test templates, the repeated-add test, and the
  test counts in the CHANGELOG are all style calls that belong to the author.

## Simplicity assessment

The implementation is still simple. The production change is: three guard
points (`src/cli/mod.rs` for `add` and `remove`, `src/cli/context.rs` for the
header pre-check) and one real rule enforced in one function
(`next_seq_after_header`), with every append routed through it. There is no new
abstraction, no new type, no configuration knob and no feature flag. Round 2
deleted more code than it added in `append_event_idempotent_in` (the dead
`path.exists()` and one level of nesting). Nothing here is over-built.
