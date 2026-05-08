# Test Plan: Local Dashboard

Generated from: docs/plans/PLAN-local-dashboard.md
Issues covered: 6

---

## Scenario 1: dashboard command is recognized by the CLI
**ID**: scenario-1
**Category**: infrastructure
**Testable after**: #1 (scaffold dashboard command and module stubs)
**Commands**:
- `koto dashboard --help`
**Expected**: exit code 0; output contains "dashboard" in the usage or description text
**Status**: passed

---

## Scenario 2: cargo build succeeds after scaffolding
**ID**: scenario-2
**Category**: infrastructure
**Testable after**: #1 (scaffold dashboard command and module stubs)
**Commands**:
- `cargo build`
**Expected**: exit code 0; no compilation errors; binary produced at `target/debug/koto`
**Status**: passed

---

## Scenario 3: ratatui and crossterm are present in Cargo.toml
**ID**: scenario-3
**Category**: infrastructure
**Testable after**: #1 (scaffold dashboard command and module stubs)
**Commands**:
- `cargo metadata --format-version 1 | python3 -c "import json,sys; pkgs=[p['name'] for p in json.load(sys.stdin)['packages']]; print(sorted(set(pkgs)))"`
**Expected**: output includes both `ratatui` and `crossterm`
**Status**: passed

---

## Scenario 4: data layer unit tests pass
**ID**: scenario-4
**Category**: infrastructure
**Testable after**: #2 (implement dashboard data layer)
**Commands**:
- `cargo test dashboard_data`
**Expected**: exit code 0; all `#[cfg(test)]` tests in `dashboard_data.rs` pass; specifically: `scan_sessions` filtering of `~`-named sessions, `stat_and_diff` add/remove/mtime-change detection, `read_session` graceful handling of parse errors
**Status**: passed

---

## Scenario 5: scan_sessions filters epoch-branched session names
**ID**: scenario-5
**Category**: infrastructure
**Testable after**: #2 (implement dashboard data layer)
**Commands**:
- `cargo test scan_sessions`
**Expected**: exit code 0; sessions with IDs containing `~` are excluded from returned list; sessions without `~` are included
**Status**: passed

---

## Scenario 6: read_session treats parse errors as unknown state
**ID**: scenario-6
**Category**: infrastructure
**Testable after**: #2 (implement dashboard data layer)
**Commands**:
- `cargo test read_session`
**Expected**: exit code 0; `read_session` on a truncated or malformed JSONL file returns `CachedSession` with `current_state = None` and `is_terminal = false` rather than panicking or returning an error
**Status**: passed

---

## Scenario 7: application state unit tests pass
**ID**: scenario-7
**Category**: infrastructure
**Testable after**: #3 (implement dashboard application state layer)
**Commands**:
- `cargo test dashboard_state`
**Expected**: exit code 0; tests for cursor movement, expand/collapse, `visible_rows` depth-first ordering, and `List`/`Detail` view mode transitions all pass
**Status**: passed

---

## Scenario 8: visible_rows returns depth-first order with correct indent
**ID**: scenario-8
**Category**: infrastructure
**Testable after**: #3 (implement dashboard application state layer)
**Commands**:
- `cargo test visible_rows`
**Expected**: exit code 0; a synthetic `SessionTree` with one coordinator and two children produces a `RowDescriptor` list where the coordinator appears first (indent 0) and children follow (indent 1), sorted failed-first then running then pending/blocked then terminal
**Status**: passed

---

## Scenario 9: cursor is bounded and does not go out of range
**ID**: scenario-9
**Category**: infrastructure
**Testable after**: #3 (implement dashboard application state layer)
**Commands**:
- `cargo test cursor`
**Expected**: exit code 0; pressing `k` when cursor is at 0 keeps cursor at 0; pressing `j` when cursor is at the last row keeps cursor at the last row
**Status**: passed

---

## Scenario 10: render layer unit test passes against TestBackend
**ID**: scenario-10
**Category**: infrastructure
**Testable after**: #4 (implement dashboard render layer)
**Commands**:
- `cargo test dashboard_render`
**Expected**: exit code 0; test using `TestBackend::new(80, 24)` draws a frame and asserts at least one cell's content at a known position without panicking
**Status**: passed

---

## Scenario 11: render_detail shows "Loading..." when detail_cache is None
**ID**: scenario-11
**Category**: infrastructure
**Testable after**: #4 (implement dashboard render layer)
**Commands**:
- `cargo test render_detail`
**Expected**: exit code 0; when `DashboardAppState.detail_cache` is `None`, the rendered buffer contains the text "Loading" somewhere in the detail panel area
**Status**: passed

---

## Scenario 12: --once flag exits 0 with empty session directory
**ID**: scenario-12
**Category**: infrastructure
**Testable after**: #5 (implement dashboard entry point and --once mode)
**Commands**:
- `KOTO_SESSIONS_BASE=/tmp/empty-sessions-$$ koto dashboard --once; echo "exit:$?"`
**Expected**: exit code 0; no tab-separated output lines printed; no error message
**Status**: passed

---

## Scenario 13: --once flag produces tab-separated output format
**ID**: scenario-13
**Category**: infrastructure
**Testable after**: #5, #6 (implement entry point and add integration tests)
**Commands**:
- Run `cargo test` in the `tests/` directory; the integration test for `--once` creates fixture sessions and asserts tab-separated output
**Expected**: exit code 0; each output line matches the pattern `<name>\t<current_state>\t<elapsed>\t<status_bucket>` where `status_bucket` is one of `running`, `done`, `failed`, `blocked`, `unknown`
**Status**: passed

---

## Scenario 14: --once output covers running and terminal sessions
**ID**: scenario-14
**Category**: infrastructure
**Testable after**: #5, #6
**Commands**:
- `cargo test dashboard --test` (the integration test in `tests/` that sets up one running and one terminal session)
**Expected**: exit code 0; at least one line has `status_bucket = running`, at least one line has `status_bucket = done` or `failed`; columns are tab-separated with exactly 4 fields per line
**Status**: passed

---

## Scenario 15: SIGINT does not leave terminal in raw mode
**ID**: scenario-15
**Category**: use-case
**Environment**: manual — requires a PTY and the ability to send signals
**Testable after**: #5 (implement dashboard entry point and --once mode)
**Commands**:
- In a real terminal: `koto dashboard`; wait for TUI to appear; press Ctrl+C
- After exit, verify: type `stty -a` and check that `echo` is on and `icanon` is on
**Expected**: terminal is restored to normal mode; shell prompt appears; subsequent commands behave normally; `stty -a` shows `echo` and `icanon` enabled
**Status**: deferred — requires PTY

---

## Scenario 16: live TUI shows sessions hierarchically
**ID**: scenario-16
**Category**: use-case
**Environment**: manual — requires a PTY for TUI rendering
**Testable after**: #4, #5 (render layer and entry point)
**Commands**:
- Initialize a koto workflow and one or more child sessions in a test directory
- Run `koto dashboard` in that directory
- Observe: coordinator session appears at indent 0, child sessions indented below it
- Press `j` and `k` to move the cursor; verify cursor moves correctly
- Press Enter on the coordinator row; verify detail panel appears at the bottom
- Press Escape; verify detail panel disappears
- Press `q`; verify TUI exits cleanly
**Expected**: session tree renders correctly with hierarchical indentation; cursor navigation works; detail panel slides up on Enter and closes on Escape; `q` exits with terminal restored
**Status**: pending

---

## Scenario 17: dashboard refreshes when session state changes
**ID**: scenario-17
**Category**: use-case
**Environment**: manual — requires a PTY and concurrent session writes
**Testable after**: #5 (implement dashboard entry point and --once mode)
**Commands**:
- Start `koto dashboard` in a directory with an active session
- In a separate terminal, advance the session state with `koto next <name>`
- Wait up to 1 second (one poll cycle at default 500ms interval)
- Observe the dashboard display
**Expected**: after the next poll tick, the row for the advanced session updates to show the new state without requiring a manual `r` refresh; the state column reflects the updated state name
**Status**: deferred — requires PTY and concurrent writes

---

## Scenario 18: --once output is usable in shell scripts
**ID**: scenario-18
**Category**: use-case
**Environment**: automatable (CI-safe, no PTY required)
**Testable after**: #5, #6
**Commands**:
- Set up a directory with two sessions (one running, one terminal) using fixture JSONL files
- Run: `koto dashboard --once | awk -F'\t' '{print NF}'`
**Expected**: every output line prints `4` (exactly 4 tab-separated fields); no lines with fewer or more fields; exit code 0
**Status**: passed

---

## Scenario 19: --interval flag overrides default poll interval
**ID**: scenario-19
**Category**: infrastructure
**Testable after**: #5 (implement dashboard entry point and --once mode)
**Commands**:
- `koto dashboard --help`
**Expected**: `--interval` flag appears in help output; description mentions milliseconds or poll interval; exit code 0
**Status**: passed

---

## Scenario 20: all cargo tests pass after integration test issue
**ID**: scenario-20
**Category**: infrastructure
**Testable after**: #6 (add dashboard integration tests)
**Commands**:
- `cargo test`
**Expected**: exit code 0; no pre-existing tests regress; dashboard-specific integration tests in `tests/` pass; `wip/` directory is clean (no leftover files)
**Status**: passed
