# Justification Scrutiny: Issue 6 — Dashboard Integration Tests

**Files reviewed**: `tests/dashboard_test.rs`, `Cargo.toml`

---

## Summary

Blocking: 0  
Advisory: 2

---

## Finding 1 — Manual JSONL append is justified (no finding)

The comment on lines 99–100 of `dashboard_test.rs` correctly explains the constraint: `koto next` deletes the session directory when it reaches a terminal state, making it impossible to use the public CLI to create a session that is both terminal and still on disk. The only viable path is to append a `transitioned` event directly to the state file.

The appended JSON shape matches the `Event` struct in `persistence.rs` (`seq`, `timestamp`, `type`, `payload`). The `read_events` function applies strict monotonic seq validation; the test correctly appends seq 3 after the two events written by `koto init` (seq 1: `workflow_initialized`, seq 2: `transitioned` to `start`). If the `Event` struct gains new required fields, this test will fail at parse time with an explicit error, not silently.

**Verdict**: the approach is defensible. Not a finding.

---

## Finding 2 — seq 3 is guaranteed (no finding)

`init_child_core` in `src/cli/init_child.rs` writes exactly two events atomically via `backend.init_state_file`: seq 1 (`WorkflowInitialized`) and seq 2 (`Transitioned { to: initial_state }`). No additional events are written by `koto init`. The hardcoded `seq: 3` is therefore guaranteed to be the correct next value.

The `read_events` parser skips trailing empty lines before processing, so the `\n` appended after the JSON event does not affect parsing.

**Verdict**: seq assumption is correct. Not a finding.

---

## Finding 3 — `is_terminal_state` reads the correct cache path (no finding)

`is_terminal_state` in `src/cli/dashboard_data.rs` reads `machine_state.template_path`, which is the compiled cache path stored in the `WorkflowInitialized` event during `koto init`. The cache location is determined by `cache.rs`:

```
$XDG_CACHE_HOME/koto/<hash>.json   (if XDG_CACHE_HOME is set)
$HOME/.cache/koto/<hash>.json      (otherwise)
```

The test sets `HOME=dir` on every subprocess and does not set `XDG_CACHE_HOME`. Both the `koto init` and `koto dashboard --once` subprocesses inherit the same environment, so they resolve the cache directory identically. The path stored in the state file and the path `is_terminal_state` reads are the same. The test passes when `XDG_CACHE_HOME` is unset (uses `dir/.cache/koto`, fully isolated), and also when it is set in the parent environment (both subprocesses inherit it, both resolve the same path — correct but outside `dir`).

**Verdict**: terminal detection is correct in all environment configurations. Not a finding.

---

## Finding 4 — Cache leaks outside `dir` when `XDG_CACHE_HOME` is set (Advisory)

When `XDG_CACHE_HOME` is set in the test runner's environment (common on Linux with systemd user sessions), the template cache is written to `$XDG_CACHE_HOME/koto/<hash>.json` — outside the `TempDir` created for the test. The test is functionally correct in this case (both subprocesses use the same cache path), but:

1. The test leaves artifacts in the developer's own cache directory.
2. Parallel test runs that compile the same template will all write to the same path, which is safe due to content-addressed caching, but may cause interference if a future change makes caching stateful.

The unit tests in `src/cli/init_child.rs` handle this correctly by using a `CacheGuard` struct that sets `XDG_CACHE_HOME` to a throwaway temp dir and restores it on drop. The integration test does not apply an equivalent guard.

**Fix**: set `XDG_CACHE_HOME` explicitly on each `koto_cmd` call, pointing to a subdirectory of `dir` (e.g., `dir.join("cache")`). This mirrors the pattern used in `init_child.rs` unit tests and makes the test fully self-contained regardless of the ambient environment.

**Severity**: Advisory. The test is functionally correct today; fixing it later requires editing only this one test file.

---

## Finding 5 — `sessions/` pre-creation is redundant (Advisory)

`dashboard_once_produces_tab_separated_output_with_running_and_terminal` calls:

```rust
std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
```

at line 97, then uses `KOTO_SESSIONS_BASE=dir/sessions`. The `LocalBackend` with `KOTO_SESSIONS_BASE` is documented to store sessions directly under that directory, and `backend.create(name)` creates subdirectories as needed. The `LocalBackend` itself creates the base directory on first use (or the first `backend.create` call). The pre-creation on line 97 is a no-op if `LocalBackend` creates it, or harmless if `LocalBackend` requires it to exist. Either way, there's no failure mode here.

The other two tests (`dashboard_help_exits_zero` and `dashboard_once_empty_dir_exits_zero_with_no_output`) do not pre-create `sessions/` and pass — confirming this line is redundant.

**Severity**: Advisory. Removing it would clean up the test. Not a blocking concern.

---

## Finding 6 — Output format assertion is structural, not over-specified (no finding)

The test checks:
- exactly 4 tab-separated fields per line
- `fields[3]` is one of `["running", "done", "failed", "blocked", "unknown"]`
- `run-wf` is in `gather` with bucket `running`
- `term-wf` is in `done` with bucket `done`

This directly tests the `classify_status` function's behavior (which is unit-tested separately in `src/cli/dashboard.rs`) through the full CLI path. The field count and bucket values are part of the `--once` output contract. Asserting them is appropriate for an integration test that aims to protect the scripting interface.

**Verdict**: the assertion level is appropriate. Not a finding.

---

## Architectural Fit

The test file uses only public CLI entry points (`koto init`, `koto next`, `koto dashboard --once`) and the `assert_cmd` crate that is already registered in `[dev-dependencies]`. It does not import any `koto::` library types. The `Cargo.toml` changes add `assert_fs` and `predicates` to dev-dependencies — both appropriate for CLI integration tests and consistent with `assert_cmd` already being present.

The raw JSONL append is the only test that touches internal format; it is constrained to the integration test that cannot use the CLI path, and the comment explains why. This does not introduce a parallel pattern — there is no other way to create a terminal-but-on-disk session.
