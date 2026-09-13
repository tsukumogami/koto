# Scrutiny: justification review, koto#236 (+#200)

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'` (commits fddc4aa, d52dd0f).
Checked against the approved scope (context-236.md), the binding review
(review-koto-explore-1.md), and the draft PR body (pr-body-236.md).

Verdict: no blocking findings. Every deviation from the obvious approach has a
real reason behind it, and the #200 claims stay inside what the evidence
supports. Seven advisory findings, mostly about behavior the PR body leaves
unsaid rather than anything it gets wrong.

Verification run for this review (scratch target dir, worktree untouched):
`cargo test --lib` gave 1572 passed. `cargo test --test claim_sidecar --test
wake_recovery --test respawn --test idempotency` gave 21, 17, 15 and 14 passed.
I did not re-run the "all 8 fail against the previous release" claim. The
reasoning holds, though: on main, `context remove` on a never-initialized
name fails inside `append_event` because the parent directory is missing,
which gives exit 3 rather than 2, so the strengthened assertion fails there.

## What checks out

**The exit code split (2 for a missing log, 3 for a headerless one) is
justified.** A missing log gets an explicit `backend.exists` check that exits
`EXIT_CALLER_ERROR` with the flat `workflow '<name>' not found` envelope
(src/cli/mod.rs:1426, src/cli/mod.rs:1486). That's the exact shape `status`
uses, which is what the approved scope asked for. A log that exists without a
header passes `exists`, then fails `backend.read_header` inside the handler
(src/cli/context.rs:26, src/cli/context.rs:149). That error lands on the
handler's existing `EXIT_INFRASTRUCTURE` mapping (src/cli/mod.rs:1443,
src/cli/mod.rs:1501). So "the session isn't there" is a caller error and "the
session is there but corrupt" is an infrastructure error, which matches how
`next` and `status` treat `StateFileCorrupted`. docs/reference/error-codes.md
says the same thing ("same as `next` above"). The reason is in the test comment
("a corrupt log is an infrastructure error", tests/state_log_integrity_test.rs)
and in the docs. The PR body states both codes but never says why they differ.
That's fine, but one clause would help a reviewer.

**Making the guard generic over the log family is justified.** The request
store's `append_under_lock` calls the idempotent append on a log whose first
line is a `RequestHeader` (src/engine/request_store/mod.rs:1256). A guard fixed
to `StateFileHeader` would have refused every request-store append, so
`append_event_in::<H>` and `append_event_idempotent_in::<H>`
(src/engine/persistence.rs:151, src/engine/persistence.rs:309) are the minimum
change. Neither is an abstraction added for its own sake.

**Reading the header before touching the store is justified.** The guard in
`append_event` alone would refuse too late, because `store.add` has already
written `ctx/<key>` by then (src/cli/context.rs:41 comes before :50). The binding
review flagged exactly this. `read_header` at src/cli/context.rs:26 and :149
closes it.

**Removing `create(true)` from `acquire_state_flock` is justified, and no caller
depended on it.** Its only caller is `append_event_idempotent_in`
(src/engine/persistence.rs:331). Its only production caller is the request
store, which refuses a missing log first (`if !path.exists()` in
`append_under_lock`, src/engine/request_store/mod.rs, just above :1256) and
creates request logs with their header through `atomic_create_rename`.
tests/idempotency.rs seeds every log with a header first (the
`write_session_file` helper), and all 14 of its tests pass. The old
`create(true)` was itself a route to an empty log: a flock that created the file
would then hand `read_last_seq` an empty file, which yields seq 1 and a
headerless line. The removal belongs to the fix, not to a separate change. The
Windows stub (src/engine/persistence.rs:435) is unchanged and was already an
error.

**The claims about #200 are honest.** The PR body offers the CLI route as a
deterministic reproduction, not as the reporter's cause. It says "Which path the
#200 reporter actually hit is unknown." It doesn't present terminal cleanup as
the cause, which the binding review asked for. "Truncation is ruled out for
local appends and rewrites" is the wording the review allowed, and the body
names the two unexercised paths (the cloud pull and a crash partway through
`rewrite_header_identity`) instead of claiming no race could do it.
`Fixes #200` holds under the guard as the review argued: every route to a lone
headerless line needs an append to a log whose first line isn't a header, and
`next_seq_after_header` (src/engine/persistence.rs:525) now refuses that. #171
stays open. The test counts (8 integration, 5 unit) match the diff.

## Advisory findings

### A1. The PR body doesn't describe what the cloud backend does now

`CloudBackend::exists` returns true when the session exists only in S3
(src/session/cloud.rs:686-692). `handle_add` then calls `read_header`, and the
cloud version first runs `sync_pull_state` (src/session/cloud.rs:751-752). That
writes into `local.session_dir(id)` with `std::fs::write`
(src/session/cloud.rs:155). If the local session directory doesn't exist, the
write fails, only a warning is printed, and the local `read_header` then fails
with a plain I/O read error. The result is exit 3 with `failed to read state
file ...`, not exit 2 and not the new "state log has no header" message. The
cloud `append_event` (src/session/cloud.rs:735) doesn't pull at all.

That isn't a regression. Before this change the same call would have created a
headerless local log and then pushed it to S3 over the good copy. It's a strict
improvement: nothing is stored and nothing is pushed. But the PR body mentions
the cloud backend only in the #200 race discussion, and the docs now promise
"exit 2 if the session has no state log". Add one sentence to the PR body.
Something like: "On the cloud backend, a session that exists only in S3 and has
no local directory is refused with exit 3 (the pull can't write locally).
Nothing is stored or pushed."

### A2. "The header check closes both" is true for #200's shape, but the emptied log is still left in place

The PR body says the cloud pull's truncate-then-write and a crash during
`rewrite_header_identity` "can leave an empty file", and that the header check
"closes both". What it closes is the route from an empty log to a headerless
`seq: 1`. The empty log itself is still unreadable, and both writers still
truncate before writing (src/session/cloud.rs:155, and `rewrite_header_identity`'s
`fs::write`). "Not in this PR" lists concurrent-writer damage and the write lock,
but not these two truncating writers. The binding review put the pull's
temp-and-rename fix in PR 2 (its Q3). Add them to "Not in this PR" explicitly so
nobody reads "closes both" as "fixed both".

### A3. The hashed idempotent path gives a less helpful error for a missing log

With a hash, `append_event_idempotent_in` takes the flock before
`next_seq_after_header` runs (src/engine/persistence.rs:331). With `create`
gone, a missing log now fails in the flock open with `failed to open state file
for lock ...: No such file or directory`. It never reaches the clearer "does not
exist; only init creates one, so nothing was appended". The unit test
(`append_event_idempotent_refuses_a_missing_log_and_creates_nothing`) asserts
only `is_err()`, so it wouldn't notice. The request store refuses a missing log
before it gets this far, so no user sees this today. Cosmetic.

### A4. A library-only fallback in claim.rs is now dead, and nothing says so

`coord_state_file_for` (src/engine/claim.rs:592-610) looks for
`<coord>/<coord>.state.jsonl`. That isn't `state_file_name`'s
`koto-<coord>.state.jsonl` (src/session/mod.rs:171-172). Otherwise it falls back
to `<child_dir>/<coord>.audit.jsonl`, a file with no header that
`append_redelegated_audit` (src/engine/claim.rs:612-629) used to create through
`append_event`'s `create(true)`. The guard now refuses that append, so
`recover_orphaned_sidecar` (src/engine/claim.rs:494, the `?` at :566) would fail
on the fallback. It's a caller that depended on append creating the file.

It isn't reachable today. `recover_orphaned_sidecar` has no production caller
(only tests/claim_sidecar.rs, whose fixture seeds a header at the name that
avoids the fallback), and every test passes. The one production path in this
family, `wake_candidates_pass` from src/cli/mod.rs:3440, checks
`coord_state_path.exists()` first (src/cli/mod.rs:3435). Still, the PR changes a
documented fallback ("so tests can still assert the call") into a hard error
without a word. Mention it in the PR body, or propose a follow-up to the
coordinator: fix the coord filename, or drop the fallback.

### A5. The existence check runs before key validation, which reverses the scope's order

The approved scope says to check "before `store.add`, after key validation". The
`backend.exists` check at src/cli/mod.rs:1426 runs before `handle_add`, and
`handle_add` validates the key inside `store.add` (`validate_context_key`,
src/session/local.rs:497). So `context add missing-session ../bad` now gets exit
2 "not found" instead of a key error. It's harmless, and arguably the better
order, since nothing is touched either way. It's just unmentioned.

### A6. "A refused write stores nothing" holds only when nothing races it

The PR body says the verbs "refuse before the context store is touched, so a
refused write stores nothing and creates no session directory". If a terminal
cleanup or a parent rewind removes the session between `read_header`
(src/cli/context.rs:26) and `store.add` (:41), `store.add`'s `create_dir_all`
recreates the session directory and writes `ctx/<key>`. Then the append (:50)
is refused. The guard still stops the #200 shape, since no headerless line is
written, so the fix holds. But the "stores nothing" sentence is absolute, and the
leftover context is exactly what the binding review worried a later-spawned
child would inherit. Qualify the sentence, or add this window to the "Not in
this PR" concurrency paragraph.

### A7. "Not in this PR" leaves out some of the review's deferrals

The binding review also deferred the scheduler's rename-aside of headerless
children, the `ENOTEMPTY` cleanup race, and the not-found exit-code mismatch
(`rewind` and `cancel` exit 1, `status` and `next` exit 2). The PR body names
none of them. Nothing is overclaimed by leaving them out. Listing them in one
line would stop a reviewer asking why a headerless child isn't cleaned up
automatically. Optional.
