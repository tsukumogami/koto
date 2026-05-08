# Completeness Scrutiny: Issue 6 — Dashboard Integration Tests

Reviewed: `tests/dashboard_test.rs` and `Cargo.toml` dev-dependencies.
Source read: `src/cli/dashboard.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard_state.rs`, `src/cli/mod.rs`.

---

## Summary

- Blocking: 1
- Advisory: 4

---

## Blocking Findings

### B1. `seq:3` hardcode is a silent correctness contract with no enforcement

`tests/dashboard_test.rs:117`

The test appends `{"seq":3,...}` by a raw string literal. This number is correct *today* because `init_child_core` always writes exactly two events (seq:1 `WorkflowInitialized`, seq:2 `Transitioned`), so the next append is seq:3. But `append_event` derives the next seq by reading `read_last_seq` from the file; it does not care what the caller believes the seq is. If `koto init` ever gains a third bootstrap event (e.g., a `SpawnEntryRecorded` or an auto-gate evaluation on init), the manually appended `"seq":3` will collide with a real event in the log, producing a duplicate seq. `derive_state_from_log` processes events in file order and the last `Transitioned` event wins, so a collision would silently cause the terminal session's state to be misread.

The fix is to read the actual last seq from the state file and increment it, or to call `append_event` using the persistence API (already used in `dashboard_data.rs` unit tests) rather than writing raw JSONL. The existing `read_last_seq` or `append_event` from `crate::engine::persistence` is available in test scope.

**Why this is blocking**: when `koto init`'s event list grows by even one event, the integration test will silently produce a corrupted fixture state and continue to pass (because `derive_state_from_log` will find the duplicate seq and use the value that happens to appear last), creating a false positive.

---

## Advisory Findings

### A1. No test for the `blocked` bucket

`dashboard.rs:35-54` shows `classify_status` returns `"blocked"` when `is_blocked == true`. `dashboard_data.rs` has unit tests that set `is_blocked`, but there is no integration test that creates a session with a recorded failed gate evaluation and then verifies that `koto dashboard --once` outputs `blocked` in the fourth column. The unit tests in `dashboard.rs` cover `classify_status` directly (`classify_status_blocked_gate_failed`), so the classification logic is tested in isolation, but the end-to-end path from a real state file through `read_session` → `classify_status` → `--once` output is not exercised. A developer who changes the gate-evaluation epoch boundary logic in `read_session` could break `is_blocked` without the integration test catching it.

### A2. No test for the `unknown` bucket

Same gap as A1 but for the `unknown` path (`current_state == None && !is_terminal`). A session that has only a `WorkflowInitialized` event (no `Transitioned`) will land in `unknown`. The third integration test only creates sessions with a known current state. The `unknown` path does appear in the `classify_status` unit test, but there is no end-to-end fixture that exercises it.

### A3. `--name` filter has no integration test

`DashboardArgs.name` is a positional-looking but optional filter: when set, `run()` skips any session whose `session_id != filter`. The integration test calls `koto dashboard --once` without `--name`, so the filter path is completely untested at the integration level. The next person reading the test suite would incorrectly conclude that `--name` is covered. There is no unit test in `dashboard.rs` for the filter either; the entire feature only has implicit coverage from the help test.

### A4. The `elapsed` field (column 3) is not validated

The test validates column indices 0 (name), 1 (state), and 3 (bucket), but never checks that column 2 (elapsed) is non-empty or follows the `format_elapsed` pattern. `format_elapsed` has its own unit tests in `dashboard.rs`, so format correctness is covered, but the integration path that derives `elapsed` from `session.mtime.elapsed()` is not checked. A regression where elapsed was accidentally omitted or emitted as an empty string would not be caught by the current test.

---

## Non-issues

- **Acceptance criteria coverage**: criteria 1–5 are met. `--help` exits 0 (test 1); `--once` on an empty dir exits 0 with no output (test 2); tab-separated format and running+terminal buckets are verified (test 3).
- **`seq` uniqueness in the running session fixture**: `koto init` + `koto next` go through the real persistence layer, so the running session's state file has no manually assigned seq values and carries no risk of collision.
- **Deterministic ordering**: the implementation sorts `all_ids` alphabetically before printing, and the test uses `find` by prefix rather than positional index, so output order changes won't break assertions.
- **CI PTY requirement**: `--once` skips `enable_raw_mode` entirely, so no PTY is needed.
- **Dev-dependencies**: `assert_cmd`, `assert_fs`, `predicates` are all present at correct versions.
