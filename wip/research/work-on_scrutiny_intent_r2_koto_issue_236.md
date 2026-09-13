# Intent scrutiny, round 2 — koto#236 state-log integrity

Reviewer: intent. Round-2 delta: commit `9f08117` ("fix(context): correct stale
append contracts and check the header first"). Baseline for the whole branch:
`5b8cfbf`.

Intent under review: no koto write path can produce a state log whose first line
is not a header, and a refused context write leaves nothing behind.

Verdict: **no blocking findings**. Six advisories, all about comment accuracy,
test coverage, and residue the branch already knows about.

## Tests run

All from the worktree root.

| Command | Result |
|---|---|
| `cargo test --test idempotency` | 17 passed |
| `cargo test --test state_log_integrity_test` | 8 passed |
| `cargo test --lib request_store` | 76 passed |
| `cargo test --lib persistence` | 57 passed |
| `cargo test` (full suite) | green, no failures |

## The five verification points

### 1. The flock still spans the whole read-then-write window — yes

`src/engine/persistence.rs:331` binds the guard to a named local (`_guard`, not
`_`), so it lives until the function returns. Everything that follows runs under
it: the header/seq read at `:337`, the scan read at `:343-377`, and the append at
`:388-397`. Every early return (`Idempotent` at `:369`, the conflict at `:371`,
and each `?`) drops the guard after the decision was made, which is the correct
order. The window did not shrink; if anything it grew, since the header read
moved inside it.

`acquire_state_flock` (`:409-424`) still opens without `create`, so the lock
acquisition itself cannot bring a log into being. The unit test
`append_event_idempotent_refuses_a_missing_log_and_creates_nothing`
(`src/engine/persistence.rs:1899`) asserts exactly that.

### 2. Idempotent hit and payload-mismatch conflict — unchanged

`src/engine/persistence.rs:368` returns `AppendOutcome::Idempotent { seq:
prior_seq }` before reaching the append block, so no write and no fsync. `:371`
still returns `EngineError::ConcurrentSubmissionConflict` with the session id
derived from the path and the caller's `state_name`. The scan body is
byte-identical to round 1 apart from indentation; only its position and the
removal of the `if path.exists()` wrapper changed.

Pinned by `tests/idempotency.rs::identical_retry_short_circuits`,
`conflicting_retry_rejected_no_write`, `idempotent_event_persists_hash_field`,
and `n32_concurrent_identical_retries_collapse_to_one_write` — all passing.

### 3. Malformed final line — the scan still tolerates it, but it no longer gets the chance

The scan's `serde_json::from_str(...) => continue` at
`src/engine/persistence.rs:350-353` is intact. But `next_seq_after_header` now
runs first (`:337`) and its last-line parse is strict:

```rust
Some(line) => {
    let val: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|e| anyhow::anyhow!("failed to parse last event line: {}", e))?;
```
— `src/engine/persistence.rs:564-566`

So a log whose *final* line is torn is now refused before the scan runs. Middle
lines are still tolerated (the scan `continue`s past them, and `next_seq` only
looks at the last non-empty line). See advisory A1 for the behaviour delta.

### 4. Header-only log (valid header, zero events) — no panic, no error

`next_seq_after_header` parses the header at `:560`, then
`lines.rev().find(|l| !l.trim().is_empty())` yields `None` and it returns `Ok(1)`
(`:562-563`). The scan's `content.lines().skip(1)` iterates zero times. The
append then writes `seq: 1`. This is the common case — every first idempotent
append on a freshly initialized log takes it — and it is exercised by
`identical_retry_short_circuits` and `idempotent_event_persists_hash_field`.

### 5. Request store (`RequestHeader`) — still behaves, torn tail still repaired first

`append_under_lock` in `src/engine/request_store/mod.rs` is unchanged in
structure: `path.exists()` → `NotFound` (`:1223`), then
`acquire_request_lock` (`:1229`), then `repair_torn_tail(&path)` (`:1234`), then
the view read, the idempotency probe, and only then
`append_event_idempotent_in::<RequestHeader>` (`:1258`). Because the tail is
repaired under the request lock before the append is called, the newly stricter
last-line parse described in point 3 cannot fire on this path.

Note the double-locking that already existed: the request store holds
`request.lock` and then `append_event_idempotent_in` takes a second flock on the
log inode. Round 2 did not change that, and the branch's own follow-up note
records it as something the lock design has to settle.

All 76 `request_store` lib tests pass, including
`a_torn_tail_is_repaired_before_the_next_append`,
`a_truncated_final_line_is_recovered_and_a_broken_middle_line_is_not`, and
`two_simultaneous_resolves_of_one_leg_leave_exactly_one_winner`.

## Advisories

### A1 — the scan's comment now overstates what it tolerates

`src/engine/persistence.rs:339-342` says the scan reads line by line "so a
malformed final line doesn't break the scan (mirrors `read_events`
tolerance)". After the reorder that is no longer what happens: the strict
last-line parse at `:565-566` rejects the log first. The tolerance the comment
describes now covers malformed *middle* lines only.

There is a real behaviour delta hiding behind the stale comment. Round 1: a torn
final line plus a matching hash on an earlier line returned `Idempotent`. Round
2: the same log errors with "failed to parse last event line". Refusing a damaged
log is the safer default, and the case is not reachable in production — the only
`Some(hash)` production caller is the request store, which repairs the tail
first, and `append_event_idempotent` with `StateFileHeader` has no production
caller today (it is referenced only from `tests/idempotency.rs`). Advisory, but
the comment should say which lines the tolerance still covers, or a future reader
will trust it.

### A2 — the improved missing-log message is unreachable on the hash path

This commit rewrote the `NotFound` error at
`src/engine/persistence.rs:532-535` to "a session's log is written when the
session is created, never by an append". On the `Some(hash)` path nobody sees it:
`acquire_state_flock` (`:409-424`) opens the file first and fails with "failed to
open state file for lock `<path>`: No such file or directory". The new wording
does reach the hash-less path, via `append_event_in` → `next_seq_after_header`.
The unit test at `:1899` asserts only `is_err()` and that no file was created, so
it does not pin either message. Worth either accepting (the flock message is
accurate, just less helpful) or giving `acquire_state_flock` the same wording.

### A3 — the exact case the reorder exists for has no test

The reorder's stated purpose is that "a headerless log is refused whether or not
the scan finds a hash match". The uncovered case is precisely that: a headerless
log that contains an event whose `idempotency_hash` matches the retry. Round-1
code skipped line 1 unseen, so the scan would find the hit on line 2+ and return
`Idempotent` on a log no reader can parse; round 2 refuses. Nothing in the suite
walks that path — the headerless cases are covered through `append_event`
(`src/engine/persistence.rs:1878`,
`append_event_refuses_a_log_whose_first_line_is_an_event`) and through the CLI
(`tests/state_log_integrity_test.rs::context_add_refuses_a_headerless_log_and_leaves_it_untouched`),
neither of which uses a hash. A short unit test — write two hashed events with no
header, retry the second, assert `state log has no header` and that the file is
byte-unchanged — would lock the ordering in against a future refactor that moves
the scan back up.

### A4 — the log is read twice per hashed append

`next_seq_after_header` reads the whole file (`:529`) and the scan reads it again
(`:343`). Both are under the same flock, so they are consistent; the cost is
double I/O on every request-store append. Having `next_seq_after_header` (or a
variant) hand the content back would remove it. Cosmetic.

### A5 — the double header check on the context verbs is correct as it stands

`handle_add` and `handle_remove` call `backend.read_header(session)` at
`src/cli/context.rs:26` and `:149`, and the append re-checks inside
`next_seq_after_header`. That is not redundancy to remove now: `store.add`
(`context.rs:41`) and `store.remove` (`:151`) both run *before*
`backend.append_event` (`:50`, `:156`), so the verb-level check is what makes
"a refused write stores nothing" true. Collapsing it needs a lock spanning both,
which the branch has already deferred.

The CLI guard added ahead of both handlers (`src/cli/mod.rs:1422-1438` for `add`,
`:1488-1498` for `remove`) refuses a session with no log at exit 2 before the
store is touched, and the comment now records why exit 2 rather than 3. That
matches `EXIT_CALLER_ERROR = 2` / `EXIT_INFRASTRUCTURE = 3` at
`src/cli/mod.rs:75,80` and the doc text in `docs/reference/error-codes.md`.
Under the cloud backend the ordering still holds: `exists` consults S3
(`src/session/cloud.rs:686`), and `read_header` pulls before delegating
(`:751`), so a remote headerless log is refused rather than appended to.

### A6 — intent residue, already recorded but only in `wip/`

Out of round-2 scope, listed so it is not lost. The header check and the append
are still two unlocked steps for the context verbs, so a truncating writer
landing between them (the cloud pull's `fs::write`, or the header rewrite at
`src/session/local.rs:683`) can still leave a lone headerless line — koto#200's
shape. And `store.add` commits before the append, so a cleanup or rewind landing
between them still strands `ctx/` content in a directory with no log.

Both are written up in `wip/followup_state-log-write-lock.md`, along with the
lock design's open questions and the `.audit.jsonl` path bug in
`engine/claim.rs`. That file lives under `wip/`, which the wip-hygiene rule
requires be deleted before the PR merges, and the note says so itself. It needs a
durable home — a filed issue — or the analysis dies with the branch.
