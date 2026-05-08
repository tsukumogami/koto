---
issue: 6
reviewer: pragmatic-reviewer
file: tests/dashboard_test.rs
---

# Pragmatic Review: dashboard_test.rs

**Blocking: 2 | Advisory: 2**

---

## Blocking

### 1. Manual state-file surgery to reach terminal state (`dashboard_once_produces_tab_separated_output...`, lines 112-126)

The test hand-crafts a raw JSONL event and appends it to the state file to drive a session into terminal state, bypassing `koto next`. This couples the test to the internal state file schema (field names, event types, timestamp format, seq numbering). Any schema change silently breaks the test. The `terminal_template` is defined specifically because `koto next` auto-advances a single unconditional transition -- just call `koto next term-wf` and assert success; the session will be terminal without manual file writes.

Fix: replace lines 112-126 with `koto_cmd(dir.path()).args(["next", "term-wf"]).assert().success()`. If `koto next` deletes the session directory on terminal (as the comment claims), that is a separate bug to fix in the engine, not to paper over in tests.

### 2. `valid_buckets` array declared inside the assertion loop (lines 157-170)

`valid_buckets` encodes the full set of allowed bucket values and is checked on every line. This is a schema-level constraint that belongs in a dedicated test (e.g., `dashboard_rejects_unknown_bucket`), not inside the mixed-coverage test that also checks specific session state. As written, adding a new bucket silently passes the loop check for all existing sessions regardless of whether the new bucket is actually reachable. The loop assertion also never exercises `failed`, `blocked`, or `unknown` buckets -- three of five values are dead test coverage.

Fix: remove the `valid_buckets` loop assertion from this test. If bucket-value validation matters, add a focused test that populates a session into each expected bucket state.

---

## Advisory

### 3. `write_template` helper called in two places but adds no value over `std::fs::write`

`write_template` is a three-line wrapper that constructs a path and calls `std::fs::write`. Its name doesn't clarify intent beyond what inline code would. It saves ~1 line per call site.

Fix: inline it at both call sites; the path construction is trivial and the result path is used directly in the `--template` arg immediately after.

### 4. `terminal_template` and `running_template` return `&'static str` but are only called once each

Both functions exist to name their fixtures, which is reasonable, but neither is reused across tests. Inlining them as `const` values or local `let` bindings would make the single-use relationship explicit.

Fix: advisory only -- the naming does add clarity for the template semantics. Leave if the team prefers named fixtures; inline if they want to keep test files flat.
