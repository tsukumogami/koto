# Lead: Does #236's mechanism hold exactly as described, and can a test show it?

## Findings

### The mechanism, confirmed by reading (main at 5b8cfbf)

The brief's description holds, with one refinement about which call creates the
directory.

1. `koto context add` dispatch, `src/cli/mod.rs:1417-1433`, calls
   `context::handle_add` directly. There's no `backend.exists` check anywhere on
   this path. Compare `handle_status` (`mod.rs:5808`), `handle_cancel`
   (`mod.rs:6294`), `handle_next` (`mod.rs:3485`) and `handle_decisions_record`
   (`mod.rs:5533`), which all bail on `!backend.exists(name)`.
2. `handle_add`, `src/cli/context.rs:16-48`, reads the content, calls
   `store.add(session, key, &content)` (line 36), then
   `backend.append_event(session, &event, ...)` (line 45). Neither step checks
   for a header.
3. `LocalBackend::add` (the `ContextStore` impl), `src/session/local.rs:493-498`,
   calls `fs::create_dir_all(&ctx_dir)`. `ctx_dir` sits inside the session
   directory, so this is the call that creates `sessions/<name>/` for a session
   that never had one. `handle_add` doesn't create the directory itself. The
   store does it as a side effect.
4. `LocalBackend::append_event`, `src/session/local.rs:189-203`, builds the state
   path and calls `persistence::append_event`. `Backend` dispatch is at
   `src/session/mod.rs:501-511`. The cloud backend, `src/session/cloud.rs:729-738`,
   delegates to `self.local.append_event` and then pushes, so it inherits the bug
   and would also push the headerless log to S3.
5. `persistence::append_event`, `src/engine/persistence.rs:139-182`: if the path
   doesn't exist, `next_seq = 1` (145-149). It then opens with
   `opts.create(true).append(true)` (159-160, mode 0600) and writes the event as
   line 1. Nothing checks for a header.
6. `read_last_seq`, `src/engine/persistence.rs:513-529`, runs
   `content.lines().skip(1)` (518). In a headerless log line 1 is the
   `context_added` event, so it gets skipped, the function returns 0, and the
   second append also gets `seq: 1`. That explains #236's duplicate `"seq":1`.
   `append_event_idempotent` has the same `skip(1)` at 323 and also opens with
   `create(true)` (373, and `acquire_state_flock` at 405).
7. The reader fails at `persistence.rs:572`
   (`failed to parse header: {}`), which yields the #236 error text.

### Tests written

New file: `tests/state_log_integrity_test.rs` (uncommitted; nothing under `src/`
or other test files was touched). It follows the conventions of
`tests/batch_scheduler_test.rs` and `tests/batch_child_cleanup_test.rs`:
`Command::cargo_bin("koto")` with `KOTO_SESSIONS_BASE=<tmp>/sessions` and
`HOME=<tmp>`, and the same parent and child batch templates.

| Test | Shape | Main |
|---|---|---|
| `context_add_refuses_a_never_initialized_session` | (a) name never initialized | FAILS |
| `repeated_context_add_does_not_build_a_headerless_log` | (a) twice; shows duplicate `seq:1` | FAILS |
| `context_add_refuses_a_blocked_batch_child_with_no_log` | (b) full #236 shape: parent ticks, `parent.B` (`waits_on: [A]`) is `blocked` with no log, `context add parent.B`; also checks the ready sibling `parent.A` still accepts context | FAILS |
| `context_add_refuses_a_session_removed_by_terminal_cleanup` | (c) standalone workflow driven to terminal without `--no-cleanup`, directory removed, then `context add` to the same name | FAILS |
| `context_add_never_leaves_an_unreadable_log` | any log `context add` leaves must pass `koto status` | FAILS, and the output carries the #236 error |
| `context_remove_refuses_a_session_dir_without_a_log` | `context remove` where the directory exists but the log doesn't | FAILS |
| `context_remove_refuses_a_never_initialized_session` | `context remove`, no directory at all | passes (see Surprises) |

The full batch setup for (b) wasn't heavy. One parent tick with a two-task
`waits_on` graph reproduces it, and the test asserts the preconditions: the
ledger says `parent.B` is `blocked`, `parent.B`'s log doesn't exist, and
`parent.A`'s log does.

Every refusal assertion checks both that the command exited non-zero and that
no state log exists afterwards. When the log does exist, the message prints its
contents and whether line 1 has `workflow`. On main it never does.

### Test command and output (fails before fix)

```
cargo test --test state_log_integrity_test
```

```
running 7 tests
test context_remove_refuses_a_never_initialized_session ... ok
test context_remove_refuses_a_session_dir_without_a_log ... FAILED
test context_add_refuses_a_never_initialized_session ... FAILED
test context_add_never_leaves_an_unreadable_log ... FAILED
test repeated_context_add_does_not_build_a_headerless_log ... FAILED
test context_add_refuses_a_session_removed_by_terminal_cleanup ... FAILED
test context_add_refuses_a_blocked_batch_child_with_no_log ... FAILED

---- context_add_refuses_a_blocked_batch_child_with_no_log stdout ----
context add on a blocked child: must not create a state log, but state log created at
/tmp/.tmpnfTmrO/sessions/parent.B/koto-parent.B.state.jsonl (first line has `workflow`: false):
{"seq":1,"timestamp":"2026-09-13T03:37:44.043Z","type":"context_added","payload":{"key":"context.md","hash":"053ccb87...","size":13}}

---- repeated_context_add_does_not_build_a_headerless_log stdout ----
first context add on a missing log: must not create a state log, but state log created at
.../sessions/never-started/koto-never-started.state.jsonl (first line has `workflow`: false):
{"seq":1,"timestamp":"2026-09-13T03:37:43.602Z","type":"context_added","payload":{"key":"context.md",...}}
{"seq":1,"timestamp":"2026-09-13T03:37:43.669Z","type":"context_added","payload":{"key":"notes.md",...}}

---- context_add_never_leaves_an_unreadable_log stdout ----
a log written by context add must be readable; status stdout=
{"command":"status","error":"state file corrupted: failed to parse header: missing field `workflow` at line 1 column 186"}

---- context_add_refuses_a_session_removed_by_terminal_cleanup stdout ----
context add after terminal cleanup: must not create a state log, but state log created at
.../sessions/finished/koto-finished.state.jsonl (first line has `workflow`: false):
{"seq":1,"timestamp":"2026-09-13T03:37:44.043Z","type":"context_added","payload":{"key":"late.md",...}}

---- context_remove_refuses_a_session_dir_without_a_log stdout ----
context remove on a directory with no log: must not create a state log, but state log created at
.../sessions/dir-only/koto-dir-only.state.jsonl (first line has `workflow`: false):
{"seq":1,"timestamp":"2026-09-13T03:37:43.602Z","type":"context_removed","payload":{"key":"context.md"}}

test result: FAILED. 1 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.51s
```

In every failing case the first command returned success (the refusal
assertion tripped on the log's existence, which is checked first). That matches
#236's step 4: `koto context add` returns success.

### Other verbs that append to a session that may lack a log

I mapped each production `append_event` caller to its enclosing function and
looked for a preceding `backend.exists` / `read_events` in that function.

These are unguarded and create a headerless log:
- `context add`: `src/cli/context.rs:45`, reached via `mod.rs:1423`. It creates
  the directory itself through `store.add`, so it reproduces from nothing.
- `context remove`: `src/cli/context.rs:147`, reached via `mod.rs:1471`.
  `LocalBackend::remove` (`local.rs:563-600`) doesn't create the directory, so
  this only reproduces when `sessions/<name>/` already exists without a log.

These are guarded (exists/read before append), subject only to a
check-then-write race:
- `rewind`: `mod.rs:2188` (guard 2128/2135).
- `next` (all the tick-time appends: `mod.rs:3600, 3846, 3982, 4341, 4429,
  4655, 5232, 5285, 5437`; guard 3485/3495). `record_notice_delivery`
  (`mod.rs:2997`) is called from the tick after that read.
- Terminal bookkeeping: `append_request_store_result_to_child` (`mod.rs:2432`,
  after read at 2688) and `append_child_completed_to_parent` (`mod.rs:2561`,
  guard on the parent at 2532).
- `decisions record`: `mod.rs:5700` (guard 5533).
- `cancel`: `mod.rs:6384` (guard 6294).
- `overrides record`: `src/cli/overrides.rs:286` (guard 139).
- `session update`: `src/cli/session.rs:494`, raw
  `persistence::append_event` (guard 486).
- `session rebind`: `src/cli/session.rs:577` (guard 526).
- `retry_failed` handling: `src/cli/retry.rs:410, 428` on the parent (inside
  the parent's tick) and `retry.rs:556` `write_rewound_event` on the child.
  Snapshots come only from children whose `read_events` succeeded
  (`retry.rs:180-186`); a cleaned-up child lands in `unknown` and is rejected.
- Wake pass: `src/engine/wake.rs:336`, raw path append to the coordinator log,
  after `read_events(coord_state_file)` at `wake.rs:471` in the same pass.

One raw-path appender I didn't verify:
- `src/engine/respawn.rs:548, 569` (`emit_respawn_event`,
  `emit_workflow_cancelled`) append to `requester_state_file` by path, and
  `persistence::append_event` will create it. I didn't trace whether every
  caller (`respawn.rs:460-503`) has read that log first. This is worth
  confirming in the write-paths lead.

The request store (`src/engine/request_store/mod.rs:1256`,
`append_event_idempotent`) writes request logs, not session state logs, so it's
out of this lead's scope.

## Implications

- A fix at the `context` verb level is enough for #236's observed shapes. The
  smallest one checks `backend.exists(session)` in `handle_add` and
  `handle_remove` before touching the store, the same guard every other verb
  uses. It should run before `store.add` so a refusal doesn't leave behind a
  `ctx/` directory either. The tests only assert that no log exists, so a fix
  that checks after `store.add` still passes, but it would leave an orphan
  directory that sets up the `context remove` shape.
- A defense-in-depth fix in `persistence::append_event` (refuse when the path
  doesn't exist, or when line 1 isn't a header) would cover every caller,
  including the unverified respawn path and any future verb. It would also
  close the check-then-write race in the guarded verbs, where terminal cleanup
  in another process can remove the directory between the `exists` check and
  the append. With `create(true)`, that race recreates a headerless log only if
  the directory still exists, which it won't after `remove_dir_all`. Given the
  ENOENT behavior below, the race window is narrow. The only verb that
  re-creates the directory is `context add`.
- `read_last_seq`'s blind `skip(1)` masks the corruption instead of surfacing
  it. Parsing line 1 as a header (or checking that it's not an event) would make
  a second append fail loudly.
- The cloud backend needs no separate fix if the local append refuses; it
  delegates first and pushes only on success (`cloud.rs:735-736`).
- For the PR, the new test file is a ready regression test. Six tests fail on
  main and should pass after a fix that refuses a session with no log in both
  `context add` and `context remove`. An add-only fix leaves
  `context_remove_refuses_a_session_dir_without_a_log` failing.

## Surprises

- `context remove` on a session with no directory already fails on main, but
  by accident: `ContextStore::remove` creates nothing, so `append_event`'s
  `create(true)` open hits ENOENT. The error comes back as an I/O failure
  (`failed to open state file ...`), not a clear "session not found". That test
  pins the refusal outcome only. It's the one test that passes on main.
- The directory is created by `ContextStore::add` (`local.rs:497`), not by
  `handle_add`. The brief's wording ("context add creates the session
  directory") is right in effect but points at the wrong function.
- The #236 error reports column 187, and ours reports 186. The difference is
  just payload length (hash and size), so it isn't significant.
- Case (c) works exactly as feared: after terminal cleanup removes
  `sessions/finished/`, a late `context add finished ...` resurrects the name as
  a headerless, permanently corrupt session.

## Open Questions

- Should the refusal be a new error code, or should it reuse whatever
  `status`/`next` return for a missing session? Those verbs exit through
  `exit_with_error` or with a code; `context add` uses `EXIT_INFRASTRUCTURE`
  (`mod.rs:1430`), which is the wrong category for caller error. This belongs to
  the agent-surface lead.
- Do the `respawn.rs` raw-path appends always run on a log that was just read?
- Should the guard live in `persistence::append_event` (every caller) or at the
  verb (the pattern every other verb follows)? Doing both costs one `exists()`
  call.

## Summary

#236's mechanism holds exactly. `koto context add` has no existence check
(`mod.rs:1417-1433`), `ContextStore::add` creates the session directory
(`local.rs:497`), `persistence::append_event` opens with
`create(true).append(true)` (`persistence.rs:159-160`), and `read_last_seq`'s
`skip(1)` (`persistence.rs:518`) yields the duplicate `seq:1`. The new
`tests/state_log_integrity_test.rs` fails on main in six cases: a
never-initialized name, the full blocked-batch-child shape, a session removed by
terminal cleanup, an unreadable log afterwards, repeated adds, and
`context remove` on a directory with no log. `context add` and `context remove`
are the only verbs that append without a guard; every other CLI writer checks
`exists`/`read_events` first, and the raw-path appends in `respawn.rs` still
need tracing.
