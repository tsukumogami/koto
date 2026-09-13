# Scrutiny: justification review, round 2, koto#236 (+#200)

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'`. Round-2 delta: commit
9f08117 (`fix(context): correct stale append contracts and check the header
first`). Checked against the round-1 justification review
(`wip/research/work-on_scrutiny_justification_koto_issue_236.md`), the PR body
on tsukumogami/koto#237, and the deferral note
`wip/followup_state-log-write-lock.md`.

Verdict: **no blocking findings.** Every one of the six refusals is defensible,
and each has a reason that survives being written down. The round-2 changes
themselves are correct: the header-first move is justified, the reworded
missing-log error is accurate, and the error-codes.md cloud caveat is not only
accurate but non-obvious and worth having. Seven advisories, five of them about
wording that is slightly off rather than wrong.

Verification run for this review (scratch `CARGO_TARGET_DIR=/tmp/koto-r2-justif`,
worktree untouched): `cargo test --lib` 1572 passed;
`--test state_log_integrity_test` 8 passed; `--test idempotency` 17 passed;
`--test claim_sidecar` 21 passed. (Round 1 recorded idempotency as 14; it is 17
and the file is untouched by this diff, so that was a mis-attributed binary, not
a change.)

## The six refusals

### 1. Test counts kept in the CHANGELOG entry -- defensible

House style confirmed, not asserted. CHANGELOG.md:110 ("Seven integration tests,
four of which fail against...") and CHANGELOG.md:149 ("...Seven integration
tests,") both end their entries the same way, and CHANGELOG.md:110 is the entry
immediately below this one. The counts are also correct: eight `#[test]`
functions in tests/state_log_integrity_test.rs (lines 144, 156, 168, 181, 198,
375, 457, 495) and five new unit tests in src/engine/persistence.rs
(`append_event_refuses_a_missing_log_and_creates_nothing`,
`append_event_refuses_an_empty_log_and_writes_nothing`,
`append_event_refuses_a_log_whose_first_line_is_an_event`,
`append_event_idempotent_refuses_a_missing_log_and_creates_nothing`,
`read_header_names_a_log_whose_first_line_is_an_event`).

### 2. Existence check before context-key validation -- defensible

src/cli/mod.rs:1430 runs `backend.exists` before `handle_add`, and key validation
happens inside `store.add` (`validate_context_key`, src/session/local.rs:497).
So `context add missing-session ../bad` reports the missing session, not the bad
key. The approved scope said "before `store.add`, after key validation", so this
does invert the scope's order, but the ordering is a pure diagnostics choice:
nothing is written either way, and naming the session first is the more useful
message because the key error would be moot. Round-1 A5 already called it
harmless. Defensible.

### 3. Two separate checks, not one helper -- defensible

The two calls genuinely do different jobs. src/cli/mod.rs:1430 and :1487 pick the
exit code (`EXIT_CALLER_ERROR` = 2, src/cli/mod.rs:80) for "the caller named a
session that isn't there", matching `status`. src/cli/context.rs:26 and :149
refuse before `store.add` changes anything, and their failure lands on the
handler's `EXIT_INFRASTRUCTURE` = 3 mapping (src/cli/mod.rs:1443, :1501). A
single helper would have to return enough structure to drive both, which is more
machinery than the duplication costs.

The stated cost is smaller than "three reads" suggests. On the local backend
`LocalBackend::exists` is a `Path::exists` stat (src/session/local.rs:81-83), and
`read_header_only` reads one line through a `BufReader`
(src/engine/persistence.rs:585-600). Only the append does a full
`read_to_string`. So the happy path is one stat, one line, one full read. On the
cloud backend the real cost is one extra S3 GET per verb: `read_header` calls
`sync_pull_state` (src/session/cloud.rs:751-752), which `read_events` was already
doing elsewhere. One GET on a verb that already does a PUT is not a cost worth
restructuring the exit codes for.

### 4. Stale comments and the dead `.audit.jsonl` fallback left in claim.rs -- defensible, with a caveat (see A5)

Confirmed unreachable: `recover_orphaned_sidecar` (src/engine/claim.rs:494) has
no caller outside tests/claim_sidecar.rs. The fallback at
src/engine/claim.rs:604-609 targets `<session_dir>/<coord>.audit.jsonl`, which
has no header and (since `create(true)` is gone) does not exist, so
`append_redelegated_audit` (src/engine/claim.rs:612) now fails at the `?` on
src/engine/claim.rs:566. Leaving a behavior change in an unreachable library path
out of scope is the right call for a targeted fix, and the PR body records it
plainly ("Anyone who wires that recovery up will need to point it at the real
coordinator log").

### 5. `append_event_in` left public -- defensible

It has no caller outside src/engine/persistence.rs (only `append_event` at :146
and `append_event_idempotent_in` at :323 use it), so this is new public API with
no consumer -- `engine` is `pub mod` in src/lib.rs:8 and `persistence` is
`pub mod` in src/engine/mod.rs:17. But the module already publishes the generic
form next to the concrete one in three other places (`read_header_only` :585,
`read_log` :679, `read_log_quiet` :687), and `read_header_only` likewise has only
in-crate callers. Following the module's own convention is a better reason than
minimizing surface by one symbol.

### 6. Two batch-parent templates kept, repeated-add test kept -- defensible

The templates are not near-duplicates. `PARENT_TEMPLATE`
(tests/state_log_integrity_test.rs:250) transitions on a `finalize` accept field;
`REWINDABLE_PARENT_TEMPLATE` (:289) adds a `gather` state in front so a rewind
has a target, and transitions on `gates.done.all_complete`. The comment at :287
says exactly why. Parameterizing them would hide the difference the second test
exists to exercise. The repeated-add test
(tests/state_log_integrity_test.rs:156) pins the reporter's exact on-disk shape
-- two events both carrying `"seq":1`, documented at :153-154 -- which is the one
observation that distinguishes #236 from a generic missing-session refusal.

## The round-2 changes

### Header check before the hash scan -- justified

The stated reason holds: before, a log with no header that happened to contain a
matching `idempotency_hash` would return `AppendOutcome::Idempotent` and report
success on a log no reader can parse, because `next_seq_after_header` only ran on
the miss path. Now src/engine/persistence.rs:336 runs first and refuses. The dead
`path.exists()` guard that wrapped the scan is correctly gone -- the flock at
:331 has already opened the file, so the path was unreachable. See A2 for the one
edge this ordering also changes.

### New missing-log wording -- accurate

"a session's log is written when the session is created, never by an append"
(src/engine/persistence.rs:534). Verified: no production code calls
`SessionBackend::append_header` (the only non-backend caller of `.append_header(`
is tests/batch_session_resolve_test.rs:279), and the batch spawner goes through
`init_state_file` exclusively -- src/cli/init_child.rs:15-17 says so and the code
matches. The old wording credited `init` alone, which was wrong for batch spawns
and session starts.

### error-codes.md cloud caveat -- accurate, and worth the line

"Under the cloud backend, remove the headerless copy from the remote store too,
or the next read pulls it back down" (docs/reference/error-codes.md:186).
Verified: `CloudBackend::read_header` (src/session/cloud.rs:751) and `read_events`
(:740) both call `sync_pull_state` unconditionally, which on a 200 does
`std::fs::write` straight over the local state path (src/session/cloud.rs:150-157).
So a headerless copy left in S3 is restored on the very next read. The claim is
exactly right for the remedy as written, too: the remedy tells the user to leave
the `ctx/` directory in place, which means the session directory still exists,
which is the condition under which that `fs::write` succeeds rather than warning.

The rest of the remedy also checks out: `LocalBackend::init_state_file`
`create_dir_all`s the session directory (src/session/local.rs:229) and only the
state-file rename is collision-guarded, so a re-`init` over a directory that
still holds `ctx/` works and the stored context survives.

### cli-usage.md "pending or blocked" -- accurate

docs/guides/cli-usage.md:459. `pending` is defined in
plugins/koto-skills/skills/koto-user/references/batch-workflows.md:70 as "Task
entry exists but no state file has been written yet", and `blocked` is
not-yet-spawned with unmet deps. Both therefore have no log, which is what the
sentence claims.

### Other round-2 comment edits -- accurate

src/session/local.rs:282-284 ("Mode 0600 matches what `append_header` sets on a
file it creates") matches `opts.mode(0o600)` at src/engine/persistence.rs:97.
src/session/mod.rs:255-259 (the trait doc) and src/engine/request_store/mod.rs:1256
(why the request store passes `RequestHeader`) are both correct.

## Advisory findings

### A1. "take the seq from the same read" reads as sharing a read it does not share

src/engine/persistence.rs:333-336 says "Check the header first, and take the seq
from the same read". The seq does come from the same read as the *header check*
(`next_seq_after_header` does both in one `read_to_string`,
src/engine/persistence.rs:529). But the scan two lines below does its own
`std::fs::read_to_string` at :343, so the function now reads the whole log twice
under the flock where the pre-round-2 code read it once on the hit path. A
reader who takes "the same read" to mean "shared with the scan" will be looking
for a variable that isn't there. Either reword to "the same read that checks the
header", or feed the scan the `content` the header check already has.

### A2. The ordering also makes a malformed last line fatal on the idempotent path

`next_seq_after_header` hard-errors on a last line that won't parse ("failed to
parse last event line", src/engine/persistence.rs:566). It now runs before the
scan, so an idempotent retry against a log with a fused or truncated final line
fails where it used to short-circuit to `Idempotent` without touching the seq.
The comment directly below still advertises the old tolerance -- "Read
line-by-line so a malformed final line doesn't break the scan"
(src/engine/persistence.rs:339-342) -- which is now only half true, since the
check above already rejected such a file.

This is narrow and arguably right (a log whose last line is unparseable cannot
accept a new event anyway, so the only case lost is the no-write retry), and
wip/followup_state-log-write-lock.md records fused lines as measured and real.
Nothing in tests/idempotency.rs covers it either way. Worth one clause in the
comment saying the tolerance now applies only to interior lines.

### A3. Round-1 A3 is unchanged: the nicer missing-log message is still unreachable on the hashed path

The flock at src/engine/persistence.rs:331 opens the file before
`next_seq_after_header` runs, and with `create` gone it fails first with "failed
to open state file for lock ..." (:417-424). So the reworded message at :534 --
the one round 2 improved -- never reaches a user on the `Some(hash)` path. The
new comment's "a log that is missing, empty or headerless is refused" is true,
but the missing case is refused by the lock, not by this check. The unit test
asserts only `is_err()`, so nothing pins it. Cosmetic; the request store refuses
a missing log before it gets here.

### A4. The claim.rs deferral's durable half is thinner than the refusal implies

The refusal cites "the PR body and wip/followup_state-log-write-lock.md". The
wip note is the deleted half -- its own opening says "copy it somewhere durable
before the branch merges" -- and it carries the actionable detail (the
`koto-<coord>` filename mismatch, "fix both before wiring the recovery up") that
the PR body compresses to one sentence. Meanwhile the source comment at
src/engine/claim.rs:588-591 still reads "we fall back to writing the audit event
into the child's directory so the test harness can still observe the call", which
this PR made false: the fallback now always errors. A future reader of claim.rs
looks at claim.rs, not at a merged PR body. One line in that doc comment, or a
filed follow-up issue, closes the gap. (No committed `wip/` path references were
introduced by this diff -- `git grep "wip/" 5b8cfbf..HEAD -- . ':!wip'` is clean,
so the hygiene rule itself is satisfied.)

### A5. CHANGELOG leads with exit 2 and never names exit 3

CHANGELOG.md:60-67 says "Both verbs now exit 2 with `workflow '<name>' not
found`" and later "A log that already has no header reports `state log has no
header` instead of a parse error", without saying that second case exits 3. A
script author reading only the CHANGELOG would branch on 2. The PR body and
docs/reference/error-codes.md both get it right; one clause in the entry would
too.

### A6. `append_event_in` is public with no consumer

src/engine/persistence.rs:151. Defensible on module precedent (see refusal 5),
but it is genuinely new public API in a crate whose CHANGELOG opens by stating
its semver discipline, and nothing outside the file calls it. If the generic pair
is meant as API, one line on `append_event_in` saying who it is for would justify
it; otherwise `pub(crate)` costs nothing.

### A7. Round-1 A7 stands: the deferral list in the PR body is still partial

wip/followup_state-log-write-lock.md's closing section names the `ENOTEMPTY`
cleanup race, the `rewind`/`cancel` exit-1-versus-2 mismatch, the scheduler's
rename-aside of headerless children, and the raw append paths in `session update`
and the wake pass. The PR body's "Not in this PR" covers the write lock and the
truncate window but none of those. Nothing is overclaimed by the omission, and
all of it lives in the wip note that gets deleted. Optional, but it is the same
durability gap as A4.
