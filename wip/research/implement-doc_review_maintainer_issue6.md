# Maintainer Review: tests/dashboard_test.rs (Issue 6)

**Blocking: 1 | Advisory: 3**

---

## Blocking

### 1. Magic JSONL structure in the terminal-session setup (line 121-126)

The test bypasses `koto next` to produce a terminal session by manually appending a raw
`transitioned` event to the state file. The shape it writes is:

```rust
r#"{"seq":{},"timestamp":"...","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"auto"}}"#
```

The `Event` struct in `src/engine/types.rs` serializes the discriminant as `"type"` with the
value coming from `EventPayload::type_name()`. `EventPayload::Transitioned` serializes its inner
fields (`from`, `to`, `condition_type`, `skip_if_matched`) as a flat `payload` object, tagged
externally by `"type"`. The test's hand-rolled JSON matches that shape today, but:

- The `"type"` key is not named `"event_type"` in the wire format (it is in the `Event` struct
  field, serialized via `#[serde(rename = ...)]` — or serde default). If someone renames the
  field or adds a `rename`, the manually written event silently becomes an `Unknown` payload and
  `derive_state_from_log` stops seeing the transition. The test will still pass if it falls back
  to `is_terminal = false`, but the assertion on `fields[3] == "done"` will fail with a confusing
  message about `status_bucket` instead of pointing to the serialization mismatch.
- There is also no comment cross-referencing `EventPayload` or `Event` in `src/engine/types.rs`.
  The next developer has no path to find the authoritative schema.

**Fix:** Replace the raw-string append with calls to `persistence::append_event` (already used in
`dashboard_data` unit tests in the same repo). That ties the test to the actual serializer and
removes the implicit contract entirely. If `append_event` cannot be used from integration tests
without a significant refactor, add a comment that names the exact struct and field (`EventPayload::Transitioned` in `src/engine/types.rs`) and explains why raw JSON is used instead.

---

## Advisory

### 2. `running_template` doc comment describes the mechanism, not the test role (line 46-47)

The comment says "requires evidence to transition, keeping the session in `gather` state after
`koto init` + `koto next`." This is accurate but describes how the template works, not what
property it establishes for tests. A developer adding a new test for a different running-state
scenario might write a second fixture that does the same thing without realizing this one already
covers it, or might change this template not knowing the timing guarantee ("stays in gather after
`koto next`") is load-bearing for the main test.

**Fix:** Add a sentence to the doc comment stating the invariant it provides to tests: "After
`koto init` + one `koto next` call, the session will remain in `gather` state because no
auto-transition fires without evidence."

### 3. `seq` derivation logic is not documented as an invariant (line 120)

```rust
let next_seq = content.lines().filter(|l| l.contains(r#""seq""#)).count() + 1;
```

This counts lines containing `"seq"` to find the next sequence number. The header line written by
`append_header` does not have a `seq` field (confirmed in `persistence.rs:12`), so the count
correctly starts at 0 for the header and counts only event lines. But this is not stated. The
next developer who reads this will think "does the header have a `seq`? What if `koto init` ever
writes more than one event? Does the seq have to be monotone from 1 or just greater than the
last?" None of that is answered by the comment above it.

The comment says "Derive the next seq from the file to avoid brittleness if koto init ever gains
additional bootstrap events" — that covers the multi-event case, but not why the line-count
heuristic is correct (no seq in header, each event line has exactly one `"seq"` key).

**Fix:** Extend the comment to state: "The header line has no `seq` field, so counting lines
with `\"seq\"` gives exactly the number of events already written. The engine expects seq to be
monotonically increasing from 1."

### 4. `write_template` returns a `PathBuf` that is only ever passed straight to `to_str().unwrap()` (lines 16-20, 105, 129)

Every call site immediately converts the returned path to a `&str`. The function's return value
adds noise without adding utility: callers have to call `to_str().unwrap()` before using it,
which can panic on non-UTF-8 paths (unlikely in tests, but not impossible across CI environments).

**Fix:** Either have `write_template` return `String` (calling `to_str().unwrap()` internally,
where the intent is visible), or document why `PathBuf` is returned (e.g., "returned as PathBuf
for easy composition with `Path`-taking helpers"). The current signature sets the next developer's
expectation that the `PathBuf` will be used as a path, not immediately stringified.

---

## What is clear

The overall structure is easy to follow: three isolated helper functions, one test per scenario,
and the `koto_cmd` helper cleanly documents the four environment knobs it sets and why. The
assertion messages on the main test (`dashboard --once should exit 0; stderr: ...`,
`"line '{}' should have 4 tab-separated fields"`) are specific enough to point directly at the
failure.
