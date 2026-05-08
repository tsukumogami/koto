# QA Validation: koto dashboard — Issue #6 (Integration Tests)

Branch: docs/local-dashboard
Date: 2026-05-08
Evaluated by: tester agent

---

## Overall Coverage Assessment: COMPLETE

All 17 automatable scenarios (1-14, 18-20) have passing test coverage. The 3 deferred scenarios (15-17) require a PTY and are correctly excluded from CI automation with documented reasons.

---

## Test Execution Results

### `cargo test dashboard` (unit + integration)

Unit tests (lib): **67 passed, 0 failed**
Integration tests (dashboard_test.rs): **3 passed, 0 failed**

All tests green. No regressions introduced.

---

## Scenario-by-Scenario Verification

### Scenario 1 — `koto dashboard --help` exits 0, output contains "dashboard"
**Status in plan**: passed
**Verification**: The integration test `dashboard_help_exits_zero` in `tests/dashboard_test.rs` runs `koto dashboard --help`, asserts `success()`, and asserts `stdout(predicates::str::contains("dashboard"))`. This test passes. Manual spot-check of `./target/debug/koto dashboard --help` confirms the usage line says "Usage: koto dashboard [OPTIONS] [NAME]".
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 2 — `cargo build` exits 0, binary produced
**Status in plan**: passed
**Verification**: `cargo build` exits 0 with no compilation errors. Binary exists at `target/debug/koto`. Running tests themselves require a successful build, so every test run independently validates this.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 3 — ratatui and crossterm present in Cargo.toml
**Status in plan**: passed
**Verification**: `cargo metadata --format-version 1 | python3 -c "..."` returns `True True` for both packages. Both are in the resolved dependency graph.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 4 — `cargo test dashboard_data` — data layer unit tests pass
**Status in plan**: passed
**Verification**: Running `cargo test dashboard_data` yields 19 tests in `cli::dashboard_data::tests`, all passing. Covered behaviors: scan_sessions filtering, stat_and_diff detection, read_session fallbacks, refresh orchestration, rebuild_roots logic.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 5 — `cargo test scan_sessions` — epoch-branched filtering
**Status in plan**: passed
**Verification**: 3 scan_sessions tests pass: `scan_sessions_filters_epoch_branched_names`, `scan_sessions_includes_all_non_epoch_branched`, `scan_sessions_empty_backend_returns_empty`. The first test creates a session "my-session" and "my-session~1", then asserts only "my-session" appears.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 6 — `cargo test read_session` — parse errors as unknown state
**Status in plan**: passed
**Verification**: 6 read_session tests pass, including `read_session_missing_file_returns_fallback` and `read_session_corrupted_file_returns_fallback`. Both assert `current_state.is_none()` and `!is_terminal` on error paths, verifying no panic.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 7 — `cargo test dashboard_state` — application state unit tests
**Status in plan**: passed
**Verification**: Running `cargo test dashboard_state` matches all tests in `cli::dashboard_state::tests`. Covered: cursor movement, expand/collapse, visible_rows ordering, List/Detail view mode transitions, task_counts, clamp_cursor, q/Ctrl+C/r keys.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 8 — `cargo test visible_rows` — depth-first order with correct indent
**Status in plan**: passed
**Verification**: 3 visible_rows tests pass. `visible_rows_depth_first_order` creates root-1, child-1-a, root-2; expands root-1; then asserts rows[0]="root-1", rows[1]="child-1-a", rows[2]="root-2". `visible_rows_children_sorted_by_priority` verifies failed → running → pending → terminal ordering by name.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 9 — `cargo test cursor` — bounded cursor
**Status in plan**: passed
**Verification**: 8 cursor tests pass. `cursor_bounded_at_zero_on_k` asserts cursor stays at 0 on 'k' when at 0. `cursor_bounded_at_last_on_j` asserts cursor stays at 0 (only one row) on 'j'. Both exactly match the scenario's expected behavior.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 10 — `cargo test dashboard_render` — TestBackend smoke test
**Status in plan**: passed
**Verification**: `render_frame_list_mode_renders_header_cells` draws a frame with `TestBackend::new(80, 24)` and asserts the top row contains "koto dashboard". The test passes without panic, matching the acceptance criteria.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 11 — `cargo test render_detail` — "Loading..." when detail_cache is None
**Status in plan**: passed
**Verification**: `render_detail_shows_loading_when_cache_is_none` sets `view_mode = Detail`, `detail_cache = None`, draws to TestBackend, then checks rows 16-23 contain "Loading". The implementation uses `"Loading\u{2026}"` (unicode ellipsis). The test asserts `contains("Loading")` which matches. Test passes.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 12 — `--once` exits 0 with empty session directory
**Status in plan**: passed
**Verification**: Integration test `dashboard_once_empty_dir_exits_zero_with_no_output` creates a `TempDir`, points `KOTO_SESSIONS_BASE` at `dir/sessions` and `HOME` at `dir`, then asserts `success()` and `stdout("")`. The test correctly isolates from the developer environment via both env vars together with `current_dir`. Test passes.
**Assessment**: Coverage matches claim. CONFIRMED.

Note: Running the command manually with only `KOTO_SESSIONS_BASE` set (without `HOME` override and `current_dir` isolation) produces sessions from the developer's own environment, which is expected. The integration test correctly uses both isolation levers.

### Scenario 13 — `--once` produces tab-separated output format
**Status in plan**: passed
**Verification**: Integration test `dashboard_once_produces_tab_separated_output_with_running_and_terminal` creates fixture sessions using the typed `append_event` API, runs `--once`, then verifies each output line has exactly 4 tab-separated fields with valid status_bucket values. Test passes.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 14 — `--once` output covers running and terminal sessions
**Status in plan**: passed
**Verification**: Same integration test as scenario-13. The test explicitly finds the "run-wf" line and asserts `run_fields[3] == "running"`, and finds the "term-wf" line and asserts `term_fields[3] == "done"`. This exactly matches the scenario's requirement for at least one running and one terminal line.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 15 — SIGINT does not leave terminal in raw mode
**Status in plan**: deferred — requires PTY
**Deferral validity**: Valid. The scenario requires sending SIGINT to a running TUI process and then observing terminal mode via `stty -a`. This cannot be automated without PTY allocation, which is not available in CI. The implementation does have a `TerminalGuard` RAII guard (`Drop` impl calls `disable_raw_mode` + `LeaveAlternateScreen`) and a signal_hook SIGINT handler — the structural mechanism is verifiable in code review, but the end-to-end behavior can only be tested manually.
**Assessment**: Deferral justified. CONFIRMED.

### Scenario 16 — Live TUI shows sessions hierarchically
**Status in plan**: deferred — requires PTY for TUI rendering
**Deferral validity**: Valid. Requires a real terminal for ratatui's CrosstermBackend to initialize correctly. The render layer is covered by TestBackend unit tests (scenarios 10-11). Full TUI interaction requires manual verification.
**Assessment**: Deferral justified. CONFIRMED.

### Scenario 17 — Dashboard refreshes when session state changes
**Status in plan**: deferred — requires PTY and concurrent session writes
**Deferral validity**: Valid. Requires launching `koto dashboard` in TUI mode, then writing to the session state from a separate process within a 500ms poll window, and observing the display update. This involves timing and terminal visibility that cannot be automated without PTY and process coordination.
**Assessment**: Deferral justified. CONFIRMED.

### Scenario 18 — `--once` output usable in shell scripts (all lines have exactly 4 fields)
**Status in plan**: passed
**Verification**: The integration test `dashboard_once_produces_tab_separated_output_with_running_and_terminal` asserts `fields.len() == 4` for every output line in a loop. This is equivalent to `awk -F'\t' '{print NF}'` producing 4 for all lines. The test logic covers the exact acceptance criterion.
**Assessment**: Coverage matches claim. CONFIRMED.

### Scenario 19 — `--interval` flag appears in help output
**Status in plan**: passed
**Verification**: `./target/debug/koto dashboard --help` output includes `--interval <INTERVAL>  Poll interval in milliseconds (default: 500)`. The `DashboardArgs` struct in `src/cli/mod.rs` defines the flag with `#[arg(long)]`. The integration test `dashboard_help_exits_zero` also validates help succeeds, though it only checks for "dashboard" as the substring, not "--interval" specifically.
**Assessment**: Coverage matches claim. The specific assertion in the integration test is broader than the scenario requires (checks "dashboard" not "--interval"), but manual and structural verification confirm the flag is present.

Minor concern: No integration test specifically asserts `--interval` appears in help output. The plan claims this passes based on `koto dashboard --help`. This is accurate but the test does not lock the flag name down.

### Scenario 20 — All cargo tests pass after integration test issue
**Status in plan**: passed
**Verification**: `cargo test dashboard` produces 67 unit tests + 3 integration tests, all passing. No regressions in any other test suite (other test binaries all show 0 failed).
**Assessment**: Coverage matches claim. CONFIRMED.

---

## Gaps

None that require fixing before merge.

The scenario-19 concern (no test specifically asserts `--interval` is in help text) is minor — the flag is clearly present, the structural definition is in `DashboardArgs`, and the integration test validates the help command succeeds. Adding a more specific assertion would improve robustness but is not required.

---

## Concerns (non-blocking)

**Concern 1 — Scenario-19 assertion breadth**: The `dashboard_help_exits_zero` integration test only checks that stdout contains "dashboard", not that `--interval` appears. If the flag were accidentally removed from `DashboardArgs`, this test would still pass while scenario-19's acceptance criterion would fail. Low risk given the flag is in the struct definition, but worth noting for future test maintenance.

**Concern 2 — Scenario-12 isolation sensitivity**: Running `koto dashboard --once` manually with only `KOTO_SESSIONS_BASE` set (without `HOME` and `current_dir` isolation) pulls in the developer's real sessions. The integration test correctly uses both isolation levers, so this does not affect test correctness. However, the scenario's listed command (`KOTO_SESSIONS_BASE=/tmp/empty-sessions-$$ koto dashboard --once`) omits the `HOME` override, which would produce non-empty output on a developer machine with existing sessions. The scenario claim is accurate when run via the test harness; the documented command in the plan is potentially misleading if reproduced literally.

---

## Summary

| Scenarios | Count | Status |
|-----------|-------|--------|
| Automatable — passed | 17 | All confirmed passing |
| Deferred (PTY) | 3 | All deferrals valid |
| Gaps requiring fix | 0 | None |
| Non-blocking concerns | 2 | Low risk |

Test commands run:
- `cargo test dashboard` — 67 unit tests + 3 integration tests, all green
- `cargo test --test dashboard_test` — 3 integration tests, all green
- Individual filter runs for `scan_sessions`, `read_session`, `visible_rows`, `cursor`, `render_detail` — all green
- Manual verification of `koto dashboard --help`, `cargo build`, `cargo metadata`
