# Intent Scrutiny — Issue 6: dashboard integration tests

## AC Coverage

| # | Acceptance Criterion | Status | Notes |
|---|---------------------|--------|-------|
| 1 | `koto dashboard --help` exits 0 | PASS | `dashboard_help_exits_zero` covers this. |
| 2 | Integration test for `--once` in `tests/` using `assert_cmd`/`assert_fs`/`predicates` | PARTIAL — see F1 | File imports `assert_cmd` and `assert_fs` but **not** `predicates`. `predicates::str::contains` at line 81 is an unresolved path — compile error. |
| 3 | `--once` asserts tab-separated output with columns `name\tcurrent_state\telapsed\tstatus_bucket` | PASS | `dashboard_once_produces_tab_separated_output_with_running_and_terminal` checks 4 fields per line and asserts field positions match. |
| 4 | Covers at least one running session and one terminal session | PASS | `run-wf` (running) and `term-wf` (terminal/"done") are both exercised and asserted. |
| 5 | `--once` runs in CI without a PTY | PASS | `--once` path bypasses TUI entirely; no raw-mode or PTY needed. |
| 6 | Render-layer unit test uses `TestBackend::new(80, 24)` | PASS | Already in `dashboard_render.rs` (Issue 4 scope, not in this file). |
| 7 | All existing tests still pass | NOT VERIFIABLE HERE | Depends on CI run; the compile error from F1 blocks this. |
| 8 | CI green, no `wip/` files | NOT VERIFIABLE HERE | Blocked by F1 at minimum. |
| 9 | E2E flow still works | PASS (design) | `--once` path is a pure read; TUI path is unchanged. |

---

## Findings

### F1 — Missing `predicates` import (BLOCKING)

`tests/dashboard_test.rs:1-3` — `predicates` crate is in `[dev-dependencies]` but not imported. Line 81 uses `predicates::str::contains(...)` directly, which is an unresolved path. This is a compile error that prevents `cargo test` from building the test binary.

Fix: add `use predicates;` (or `use predicates::prelude::*;`) at the top of the file, or inline the predicate as `.stdout(predicates::str::contains("dashboard"))`. Alternatively, just `.stdout(predicates::str::contains("dashboard"))` works if `predicates` is added as an explicit `use` item: `use predicates::prelude::PredicateBooleanExt;` is not needed here — simply `use predicates;` at the top is sufficient since the call is fully qualified.

---

### F2 — Manual JSONL append relies on knowledge of post-init seq count (ADVISORY)

`tests/dashboard_test.rs:108-120` — The comment says "header + seq 1 + seq 2" after `koto init`, which is correct today. But the seq assumption is fragile: if `koto init` ever appends an extra event (e.g., a future audit event), the hardcoded `"seq":3` will create a sequence gap. `read_events` treats a non-final gap as a corruption error, so this would silently make `is_terminal` fall back to `false` and the assertion at line 190 (`term_fields[3] == "done"`) would fail with a confusing message.

Fix: either derive the next seq by reading the current last seq from the file before appending, or use `backend.append_event(...)` via a test-only helper. The risk is bounded because `koto init` is stable, so this is advisory rather than blocking.

---

### F3 — `dashboard_once_empty_dir_exits_zero_with_no_output` does not create the sessions dir (ADVISORY)

`tests/dashboard_test.rs:85-92` — `koto_cmd` sets `KOTO_SESSIONS_BASE` to `dir/sessions`, but this test never creates `dir/sessions/`. `LocalBackend::with_base_dir` calls `backend.list()` which internally calls `fs::read_dir` on the base dir; if the dir doesn't exist `list()` will likely return an error or an empty iterator depending on the implementation. If `list()` propagates the I/O error, `dashboard --once` exits non-zero and the `.success()` assertion fails.

Reading `LocalBackend::list()` shows it calls `fs::read_dir`, which returns `Err(NotFound)` for a missing directory. The current `refresh()` propagates that error, which would cause `dashboard --once` to exit with a non-zero code — contradicting the test assertion. Either the test must create the directory first (as the third test does at line 97), or `refresh()` must treat a missing base dir as an empty session set.

This is potentially blocking if the directory doesn't exist and the error propagates to a non-zero exit. Investigation required.

---

### F4 — `is_terminal` may be `false` for `term-wf` despite JSONL showing `"done"` state (ADVISORY, design note)

`src/cli/dashboard_data.rs:211-216` — `is_terminal` is determined by reading the compiled template JSON from `template_path` stored in the `WorkflowInitialized` event. That path is in `$HOME/.cache/koto/`. The test sets `HOME=dir`, so the cache is under `{tmpdir}/.cache/koto/` — the same temp dir used throughout the test. Since `koto init` compiles and caches the template before writing the `WorkflowInitialized` event, the cache file will be present when `dashboard --once` runs. This is sound.

However: the `dashboard_data` unit test `read_session_with_transition_returns_state` already documents that `is_terminal` is `false` when the template path doesn't exist in tests (line 807). The integration test avoids this because `HOME` is redirected into the temp dir where the real cache resides. This is a correct and deliberate design.

---

## Summary

- **Blocking**: 1 (F1 — missing `predicates` import, compile error)
- **Advisory**: 2 (F2 — fragile hardcoded seq number; F3 — missing sessions dir may cause non-zero exit in `dashboard_once_empty_dir_exits_zero_with_no_output`)
- **Design note**: 1 (F4 — `is_terminal` resolution is sound given `HOME` redirection)
