# Architect Review: dashboard_test.rs (Issue 6, commit fd471cb)

**Blocking: 1 | Advisory: 1**

---

## Blocking

### 1. Direct JSONL write bypasses `append_event`, creating a fragile state-construction path

`tests/dashboard_test.rs:119-126` -- The test creates a terminal session by reading the state file as a raw string, computing a `next_seq` with a substring count (`filter(|l| l.contains(r#""seq""#))`), then appending a hand-rolled JSON line directly.

This bypasses `persistence::append_event`, which is the canonical write path. Two concrete risks that compound:

- The `append_event` function reads the last line's parsed `seq` field to determine the next sequence number (`read_last_seq`). The test's counter-based approach (`count + 1`) will diverge if `koto init` ever emits a bootstrap event that lacks a `seq` field (e.g., the header line, which intentionally has no `seq`). The filter `l.contains(r#""seq""#)` happens to work today because the header has no `seq`, but it is one additive event type away from miscounting.
- The hand-rolled JSON string hard-codes `"condition_type":"auto"`. If `EventPayload::Transitioned` gains a new required field (or the `condition_type` value contract narrows), the test produces a JSONL line that the production reader (`read_events`) would classify as `EventPayload::Unknown`, causing `dashboard_data::refresh` to report `current_state: None` rather than `"done"`, silently breaking the assertion at line 197 without any indication that the state file is malformed.

The existing `integration_test.rs` never writes JSONL directly; it always drives state transitions through the CLI. The dashboard tests should do the same. The correct approach: use a template where the `gather` state has an auto-transition target (like `terminal_template` already does) and drive the terminal session forward with `koto next <name>` calls instead of manual JSONL writes.

Fix: replace the JSONL-write block (lines 111-126) with `koto_cmd(dir.path()).args(["next", "term-wf"]).assert().success()`. The `terminal_template` is already structured to auto-advance on `koto next` since `start` has an unconditional transition to `done`. No test infrastructure change needed.

---

## Advisory

### 2. `koto_cmd` omits `XDG_CACHE_HOME` isolation that the canonical helper in `integration_test.rs` does not have either -- but `dashboard_test.rs` sets it while `integration_test.rs` does not

`tests/dashboard_test.rs:12` -- `koto_cmd` sets `XDG_CACHE_HOME` to redirect the template compile cache into the temp dir. The equivalent helper in `integration_test.rs:10-18` does not. This is a **parallel pattern**: two `koto_cmd` helpers with slightly different isolation guarantees in the same test suite.

This is advisory because the code works and the isolation difference is harmless right now (`XDG_CACHE_HOME` matters only for template caching, not session storage). But if a third test file is added and its author copies one of the two helpers, the discrepancy will spread. The fix is to consolidate both helpers into a shared `tests/common/mod.rs` (or `tests/helpers.rs`) with `XDG_CACHE_HOME` isolation included, and have both test files import it.

---

## Structural questions answered

**Subprocess vs library level**: Correct choice. `dashboard.rs`'s `--once` path exercises the full stack (session discovery, event log replay via `persistence::derive_machine_state`, `classify_status`). Testing at the CLI boundary catches regressions in the dispatch layer that unit tests of individual functions would miss.

**`KOTO_SESSIONS_BASE` as the isolation interface**: Correct. `build_local_backend()` in `src/cli/mod.rs:536-543` explicitly documents this env var as the test isolation hook and implements it. The test is using the documented interface, not an accidental side channel.

**Public CLI contracts vs internal state**: With the exception of the direct JSONL write (Blocking #1), the tests invoke only public CLI verbs (`init`, `next`, `dashboard --once`). The assertion on field count and bucket values tests the tab-separated contract documented in `dashboard.rs:96-99`, which is the right surface to pin.

**Output format coupling**: The test asserts 4 tab-separated fields and validates `status_bucket` against the enumerated set `["running", "done", "failed", "blocked", "unknown"]`. This set is derived from `classify_status` in `dashboard.rs:35-54`. The test and the implementation are consistent; no drift.
