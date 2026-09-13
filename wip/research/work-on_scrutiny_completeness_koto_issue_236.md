# Scrutiny: completeness, koto#236 (+ #200)

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'` (HEAD d52dd0f). Checked against
the approved scope (context-236.md, "Approved scope" items 1-7) and the binding
review's PR-1 requirements (sections 1, 3, 4). PR body: pr-body-236.md.

Verdict: no blocking findings. Every required criterion has an implementation and,
where the scope asked for one, a test. Every PR-body claim I could check against
the diff or by running tests is true. Four advisory findings are listed at the end.

## What I ran

- `cargo test --test state_log_integrity_test --test doc_names` on HEAD: 8/8 and
  24/24 pass.
- `cargo test --lib persistence` on HEAD: 57 pass, including the 5 new guard tests.
- Base-source check: extracted `git archive 5b8cfbf`, copied HEAD's
  `tests/state_log_integrity_test.rs` into it, ran it: **0 passed, 8 failed**. This
  confirms the PR-body and CHANGELOG claim "all 8 fail against the previous release".
- Full `cargo test --no-fail-fast` on HEAD: 47 test binaries, 2295 passed,
  0 failed, 4 ignored, exit 0. Nothing that used to rely on `append_event`
  creating a log is broken (claim, wake, respawn, dashboard, request store and the
  batch suites all pass).

## Criterion-by-criterion

### Scope 1: context add/remove refuse before touching the store

- Implemented at src/cli/mod.rs:1422-1434 (add) and src/cli/mod.rs:1484-1494
  (remove): `if !backend.exists(&session)` exits with
  `{"error":"workflow '<name>' not found","command":"context add|context remove"}`
  and `EXIT_CALLER_ERROR` (2), before `context::handle_add`/`handle_remove`.
- `LocalBackend::exists` checks for the state file, not the directory
  (src/session/local.rs:81-83), so a directory-only session is refused too.
- Tested: tests/state_log_integrity_test.rs:103-125 (`assert_not_found`: exit 2,
  exact error body, `command` field, no log) and :130-137 (`assert_no_session_dir`),
  used by the tests at :144, :156, :168, :181, :375, :457, :495.
- Deviation from the order the scope stated (advisory A1 below): the scope says
  "after key validation", but key validation happens inside `store.add`/`store.remove`
  (src/session/local.rs:494, :564), which now runs after the existence check.

### Scope 2 / review section 1: header guard in append_event

- `append_event` delegates to `append_event_in::<StateFileHeader>`
  (src/engine/persistence.rs:145-161). `next_seq_after_header::<H>`
  (persistence.rs:525-567) refuses a missing file, an empty file, an empty first
  line, and a first line that doesn't parse as `H`, before any open-for-append.
- The append opens with `.append(true)` and no `create`
  (persistence.rs:170-175, :385-388). `acquire_state_flock` no longer creates the
  file (persistence.rs:405-422).
- `read_last_seq`, which skipped line 1 without looking at it, is gone, replaced by
  `next_seq_after_header`.
- `append_event_idempotent` goes through the same guard (persistence.rs:303-305,
  :323, :376). The request store now passes its own header type
  (src/engine/request_store/mod.rs:70, :1256), so request-log appends validate
  against `RequestHeader`.
- Unit tests (persistence.rs:1843-1929): missing log
  creates nothing; empty log stays empty; event-first log is refused and left
  byte-for-byte unchanged; idempotent append to a missing log creates nothing;
  reader names the headerless log. All five pass.

### Scope 3: clearer no-header error plus documented remedy

- `header_parse_failure` (persistence.rs:627-641) turns an event-first line into
  "state log has no header: its first line is a `<type>` event (seq N) ...". Other
  parse failures keep the serde message.
- Tested at the unit level (`read_header_names_a_log_whose_first_line_is_an_event`)
  and end to end through `koto status` (tests/state_log_integrity_test.rs:234-243).
- Remedy documented at docs/reference/error-codes.md:180-186. I checked it:
  `init_state_file` calls `create_dir_all` on the session directory
  (src/session/local.rs:231), so leaving `ctx/` in place doesn't block a later
  `koto init` or scheduler spawn.

### Scope 4 / review section 3: tests

| Required | Where | Status |
|---|---|---|
| exit 2 and the not-found message | state_log_integrity_test.rs:103-118 | done |
| no ctx/ file after refusal | :130-137 (no session dir at all), :229-232 (`context exists` false for the headerless case), :442-445 (B has no leftover key) | done (see A2) |
| no session dir for a never-initialized name | :150, :164, :174 | done |
| blocked child: complete A, tick parent, B spawns, is readable, no leftover context | :427-445 | done |
| rewind test | :494-532 | done |
| persistence unit tests: empty and event-first refused, nothing written | persistence.rs guard tests | done |
| delete or fix `context_add_never_leaves_an_unreadable_log` | not in the file any more | done (deleted) |

The concurrency acceptance tests in section 3 belong to PR 2 (section 4), so they
aren't expected here.

### Scope 5: the 13 integration tests

tests/integration_test.rs:3065-3071 replaces `create_session_dir` with
`init_context_session`, which runs `init_workflow`. Fifteen call sites switched.
Twelve of those tests write context. `context_add_rejects_invalid_key` would also
break without init, because the existence check now runs before key validation.
That makes 13 tests that need the change, matching the PR body. The other two
(`context_exists_returns_exit_1_when_missing`, `context_get_missing_key_returns_error`)
never write, and switching them does no harm.

### Scope 6: docs and skills

- koto-user command-reference.md:782 (add, exit 2) and :844-846 (remove, exit 2):
  done.
- docs/reference/error-codes.md:222-233 (new "context add and context remove"
  section) and :180-186 (headerless log): done.
- docs/guides/cli-usage.md:459 (context add exit 2): done. The `context remove`
  section (cli-usage.md:512-532) states no exit behavior (advisory A3; the scope
  named only :452).
- koto-user batch-workflows.md:78 (pending/blocked child has no session; use task
  `vars` or parent context): done.
- `cargo test --test doc_names`: 24/24 pass.

### Scope 7: CHANGELOG and PR body

- CHANGELOG.md:47-71 is under `## [Unreleased]` (line 9) / `### Fixed` (line 45),
  and it calls out the exit-2 behavior change. Done.
- PR body has `Fixes #236`, `Fixes #200`, and keeps #171 open ("It relates to
  #171, which stays open"). It limits the truncation claim to local rewrites and
  appends and says the cloud pull and crash-mid-rewrite paths are closed by the
  header guard. It says "Which path the #200 reporter actually hit is unknown" and
  doesn't name terminal cleanup as #200's cause. It calls out the behavior change
  under "Behavior change". All done.

### Review section 4: PR-1 scope only

No write lock, no atomic rewrites, no cloud-pull change, no scheduler rename-aside
in the diff. Scope held.

## PR-body claims checked

| Claim | Evidence | Verdict |
|---|---|---|
| add/remove exit 2 with `workflow '<name>' not found`, refuse before the store is touched, create no session dir | mod.rs:1422-1434, :1484-1494; tests pass | true |
| read the header before changing the store; headerless log refused (exit 3) and left unchanged | context.rs:23-26, :147-149; mod.rs:1440 `EXIT_INFRASTRUCTURE`; test :198-244 | true |
| append_event / append_event_idempotent no longer create the file; acquire_state_flock no longer creates it | persistence.rs:170-175, :385-388, :405-422 | true |
| generic over the log family because the request store shares the path | `append_event_in::<H>`, `append_event_idempotent_in::<H>`; request_store/mod.rs:1256 | true |
| `read_last_seq` replaced by `next_seq_after_header`, which validates line 1 | persistence.rs:525-567 | true |
| new no-header message text | persistence.rs:627-641; unit and integration tests | true |
| 13 integration tests fixed via the shared helper | integration_test.rs:3065-3071 (see scope 5) | true |
| docs already promised non-zero exit | old cli-usage.md line: "Exits non-zero if the session doesn't exist" | true |
| 8 tests, all fail on the previous release, all pass now | base-source run 0/8; HEAD 8/8 | true (measured) |
| five unit tests in persistence.rs, the listed cases | persistence.rs guard tests | true |
| skills edits (command-reference, batch-workflows); koto-adhoc and koto-author unchanged | diff touches only the two koto-user reference files | true |
| ~7,700 racing writes, none lost a header | only in wip/research/explore_state-log-integrity_r1_lead-200-repro.md:265 and wip/explore_state-log-integrity_findings.md:27 | not verifiable from the diff (A4) |
| header guard "closes" the cloud-pull and crash-mid-rewrite paths | see A4 | true for a leftover empty file; overstated for a truncate that races the append |

## Advisory findings

**A1. The existence check runs before key validation, not after.** The scope said
to check "before `store.add`, after key validation". Key validation lives in
`LocalContextStore::add`/`remove` (src/session/local.rs:494, :564), and the CLI
check at src/cli/mod.rs:1422 runs first. So `koto context add ghost ../escape.md`
now reports `workflow 'ghost' not found` (exit 2) instead of the invalid-key
error. Both are exit-2 caller errors, nothing is written either way, and
`context_add_rejects_invalid_key` still passes because it inits first. It's a
small change in which error wins, and no test pins it. Either note it, or call
`validate_context_key` before the existence check.

**A2. No test covers `context add` to a directory that has no log.** The
"no ctx/ file" requirement is asserted by the no-session-dir tests and by
`context exists` in the headerless and blocked-child tests. But the one test with
an existing directory and no log (tests/state_log_integrity_test.rs:181-188)
exercises only `context remove`, and it doesn't check the ctx store afterwards. An
`add` variant that asserts `ctx/<key>` is absent would pin the "refuse before
`store.add`" ordering for the case where the directory already exists. The code
does the right thing: the check comes before `handle_add`.

**A3. cli-usage.md's `context remove` section doesn't mention exit 2.**
docs/guides/cli-usage.md:512-532 says nothing about exit codes, while `context add`
(:459) and the koto-user command reference (:844-846) now document exit 2 for
remove. The scope only named :452, so this isn't required. One sentence would make
the two sections consistent.

**A4. Two PR-body statements need care.** (a) "We also raced ... for about 7,700
writes" rests on exploration output that lives only in wip/, which cleanup
deletes before merge. It's fine in a PR body, but a reader can't verify it from
the diff. (b) The guard is check-then-open: `next_seq_after_header` reads the
file, and a separate `OpenOptions::append` opens it (persistence.rs:160-175). A
cloud pull whose `fs::write` truncates between those two steps can still receive a
headerless append. So the guard closes the *leftover empty file* case (crash
mid-rewrite, or a pull whose write failed), which is what the binding review
asked the body to say. It narrows, rather than closes, the concurrent-truncate
window. The "Not in this PR" line "Concurrent writers can still damage a log
without losing its header" is slightly too strong for that window. Suggested
wording: "... closed by the header guard when they leave an empty log; a pull that
truncates while an append is in flight is the write-lock follow-up's job."
Advisory, because the body follows the phrasing the binding review specified, and
the follow-up (PR 2) owns the pull fix.
