# Scrutiny: completeness, round 2, koto#236 (+ #200)

Reviewed `git diff 5b8cfbf..HEAD -- . ':!wip'` at HEAD 9f08117, with the round-2
delta `git diff fddc4aa..HEAD -- . ':!wip'` (commits d52dd0f, 1b6dcd1, 9f08117)
read line by line. Checked against the approved scope (context-236.md items 1-7),
the binding review's PR-1 requirements (review-koto-explore-1.md sections 1, 3, 4)
and the PR body (pr-body-236.md).

Verdict: **no blocking findings.** All seven claimed fixes are present and correct
in the code. No acceptance criterion regressed. Three advisories.

## What I ran

- `cargo test --no-fail-fast` on HEAD (`CARGO_TARGET_DIR=/tmp/koto-r2-target`):
  **2295 passed, 0 failed, 4 ignored**, 47 test binaries, exit 0. Same totals as
  round 1, so the delta broke nothing.
- `tests/state_log_integrity_test.rs`: 8/8 pass. `--test doc_names`: passes as
  part of the full run.
- `cargo fmt --check`: clean.
- `cargo clippy --all-targets`: two `invalid_regex` errors at
  `src/template/types.rs:3336` and `:3341` — deliberately invalid regexes inside
  pre-existing tests, untouched by this branch and present on base. Not this PR's.

## The seven claimed fixes, one by one

**1. `src/session/local.rs` no longer claims appends create state files — DONE.**
`src/session/local.rs:281-284` now reads "Creating the state file is this
function's job: appends refuse a log that isn't already there. Mode 0600 matches
what `append_header` sets on a file it creates." That is the round-1 blocking
finding (maintainer B1) and the comment is now consistent with
`append_event_in`'s no-`create` open at `src/engine/persistence.rs:170-175`.

**2. `SessionBackend::append_event` trait doc states the contract — DONE.**
`src/session/mod.rs:255-259`: "Fails, writing nothing, unless the log already
exists and its first line is a header. Creating a session's log belongs to
[`SessionBackend::init_state_file`]; an append that created one would leave a log
no reader can parse (koto#236)." That closes architect A1. The contract matches
both implementations: `LocalBackend::append_event` funnels into
`append_event_in::<StateFileHeader>` (`persistence.rs:145-160`), and the cloud
backend delegates to local before pushing.

**3. `append_event_idempotent` doc no longer says "appended unconditionally" — DONE.**
`src/engine/persistence.rs:275-276` now reads "`None`: identical to
[`append_event`]. The event is appended with no hash field, and the same header
check applies." Accurate: the `None` arm at `persistence.rs:322-325` delegates to
`append_event_in::<H>`, which runs the guard.

**4. The header check runs BEFORE the hash scan, and the dead `path.exists()` is gone — DONE.**
`next_seq_after_header::<H>(path)?` is now at `persistence.rs:337`, above the scan
loop that starts at `persistence.rs:339-345`; the old call site after the loop is
gone. The `if path.exists()` wrapper (dead on unix because the flock open at
`persistence.rs:414-424` already fails on a missing file) is removed, and the scan
body is dedented one level. The comment at `persistence.rs:333-336` gives the
reason: an idempotent hit would otherwise report success on a log no reader can
parse. Two knock-on effects, both correct:
- The doc at `persistence.rs:307-308` ("refuses a log whose first line isn't an
  `H`") is now literally true, which is what maintainer A4 asked for. It was
  overstated before the reorder; the reorder is the better of the two fixes.
- The scan's "Skip the header line" comment is replaced by "Line 1 is the header,
  checked above, so events start at line 2" (`persistence.rs:340-341`), which was
  the other half of A4.

**5. The missing-log error no longer credits only `init` — DONE for the user-facing message.**
`persistence.rs:533`: "state file {} does not exist, so nothing was appended; a
session's log is written when the session is created, never by an append."
`append_event_refuses_a_missing_log_and_creates_nothing`
(`persistence.rs:1846-1854`) asserts only `contains("does not exist")`, so the
reword doesn't break it, and it still passes. See advisory A1 for the one
remaining "only init" in a comment.

**6. Both CLI and request-store comments — DONE.**
- `src/cli/mod.rs:1422-1429` now says why exit 2: "Exit 2, as `status` does: the
  caller named a session that isn't there. A log that exists but can't be read is
  exit 3, reported by the handler." That matches the code:
  `EXIT_CALLER_ERROR` at `src/cli/mod.rs:1436`, `EXIT_INFRASTRUCTURE` for the
  handler error at `src/cli/mod.rs:1447`. The same comment also broadens "only
  init" to "init, session start, a batch spawn" (maintainer A1 for this site).
  `context remove` at `src/cli/mod.rs:1488-1489` cross-references add, which is
  enough.
- `src/engine/request_store/mod.rs:1255-1256`: "A request log's first line is a
  RequestHeader, not a session header, so the append checks it against that
  type." That closes maintainer A3's main half.

**7. Both doc edits — DONE.**
- `docs/guides/cli-usage.md:459` now lists "it's a batch child that is still
  `pending` or `blocked`" alongside never-initialized and cleaned-up (maintainer
  A7). It also gained an exit-2 sentence for `context remove` at
  `docs/guides/cli-usage.md:527-529`, which closes my own round-1 advisory A3.
- `docs/reference/error-codes.md:186` now says *why* `ctx/` stays ("so the context
  already stored there survives") and adds the cloud caveat ("Under the cloud
  backend, remove the headerless copy from the remote store too, or the next read
  pulls it back down"). I re-verified the caveat against the code:
  `CloudBackend::exists` falls back to S3 (`src/session/cloud.rs:685-692`) and
  `read_header` calls `sync_pull_state` (`src/session/cloud.rs:750-753`), which
  overwrites the local file from S3 (`src/session/cloud.rs:149-158`). Accurate.

## Criteria re-check after the delta

Nothing in the round-2 delta touches the CLI refusal, the guard's placement in
`append_event_in`, the tests, the integration-test helper, or the skills files, so
the round-1 criterion-by-criterion pass stands
(`wip/research/work-on_scrutiny_completeness_koto_issue_236.md`). Re-confirmed the
pieces the delta could have disturbed:

| Criterion | Where | Status after delta |
|---|---|---|
| add/remove refuse before the store, exit 2, exact message | `src/cli/mod.rs:1430-1438`, `:1490-1498` | unchanged, tests pass |
| header read before the store changes | `src/cli/context.rs:23-24`, `:147-148` | unchanged |
| guard in `append_event` / `append_event_idempotent` | `persistence.rs:160`, `:337` | now stricter, not weaker |
| `acquire_state_flock` doesn't create the log | `persistence.rs:411-417` | unchanged |
| `read_last_seq` gone | repo-wide grep: no hits | still gone |
| 8 integration + 5 unit tests | `tests/state_log_integrity_test.rs`, `persistence.rs:1845-1932` | 8/8 and 5/5 pass |
| 13 integration tests fixed | `tests/integration_test.rs:3065-3071` | unchanged, suite green |
| docs + skills, `doc_names` | cli-usage, error-codes, koto-user refs | doc_names passes |
| CHANGELOG `[Unreleased]` / `### Fixed`, behavior change called out | `CHANGELOG.md:47-72` | added in this delta, correct section, states exit-2 change |
| PR-1 scope only (no write lock, no cloud-pull fix, no rename-aside) | whole diff | held |

## PR body vs. the code after the delta

The delta doesn't invalidate any PR-body claim, and it makes one of them more
accurate:

- "`append_event` and `append_event_idempotent` only append when the log already
  exists and its first line parses as a header" (pr-body lines 8-10) was slightly
  optimistic before, because a hash hit short-circuited above the header check.
  After the reorder it is true for every path through
  `append_event_idempotent_in`.
- "Both also read the session header before changing the store, so a log that
  exists but has no header is refused (exit 3)" — still true
  (`src/cli/context.rs:23`, `src/cli/mod.rs:1447`).
- "Under the cloud backend, a session that exists only in S3, with no local copy,
  now exits 3 with a read error" — verified: `exists` returns true from S3,
  `read_header` pulls, `fs::write` into a missing session dir fails with a
  warning, and `local.read_header` then errors into `EXIT_INFRASTRUCTURE`.
- The quoted headerless-log message and the "8 tests / five unit tests" counts are
  untouched by the delta and still match the code and the test run.
- The body's "Not in this PR" paragraph already carries the check-then-open
  caveat (pr-body lines 48-54), which was my round-1 advisory A4(b). No change
  needed.

The CHANGELOG entry added in this delta (`CHANGELOG.md:47-72`) tracks the same
facts: exit 2, refusal before anything is stored, the append-level guard, the
`seq: 1` reuse, the new no-header message, the error-code reference, and "Eight
integration tests, all of which fail against the previous release, and five unit
tests." The 8-fail-on-base half was measured in round 1 against
`git archive 5b8cfbf`; the delta changed no test, so it still holds.

## Advisory findings

**A1. One "only init creates a log" survives, in the flock comment.**
`src/engine/persistence.rs:411`. Maintainer A1 named three sites; the error
message (`:533`) and the CLI comment (`src/cli/mod.rs:1422-1423`) were both
broadened to "when the session is created", and this one wasn't. It's a code
comment, not user-facing, and the sentence is still directionally right, but the
branch now says two different things about the same fact. One-line fix, same
wording as the other two.

**A2. Nothing pins the new ordering inside `append_event_idempotent_in`.**
The delta's only behavior change is that a headerless log whose later line carries
a matching idempotency hash now errors instead of returning
`AppendOutcome::Idempotent`. No test covers it:
`append_event_idempotent_refuses_a_missing_log_and_creates_nothing`
(`persistence.rs:1899-1916`) uses a missing file, which the flock open rejects
first and which the pre-delta code also rejected; `tests/idempotency.rs` only uses
valid logs. So moving `next_seq_after_header` back below the scan would keep the
whole suite green. A six-line unit test (headerless file whose second line has
hash `h`, `append_event_idempotent(..., Some(h))` must be `Err` and the file
unchanged) would hold the fix in place.

**A3. The reorder narrows torn-tail tolerance on the idempotent path.**
`next_seq_after_header` hard-errors on a final line that isn't valid JSON
(`persistence.rs:565-566`), while `read_log_inner` recovers a truncated final line
by design (`persistence.rs:691-724`), and for the multi-writer request log a torn
tail is documented as normal operation, not a crash (`persistence.rs:660-671`).
Before the delta, a hash-hit retry against a log with a torn tail returned
`Idempotent`; now it fails first. The exposure is small — every non-hit append,
which is the common case, already failed the same way both before this branch
(`read_last_seq` at base `persistence.rs:513-528` had the identical
`from_str(...)?`) and after it — so this is a pre-existing sharp edge that the
reorder extends to one more case rather than a new defect. Worth a line in the
write-lock follow-up (`wip/followup_state-log-write-lock.md`), where torn tails
and single-write appends already live.

## Also checked, unchanged and fine

- No `path.exists()` remains on any append path
  (`persistence.rs:88` is `append_header_line`'s first-write detection, which is
  correct and untouched).
- The scan still tolerates a malformed interior line by `continue`
  (`persistence.rs:346-349`), matching its comment.
- `cargo fmt` clean after the dedent, so the reorder's whitespace churn is real
  formatting, not drift.
