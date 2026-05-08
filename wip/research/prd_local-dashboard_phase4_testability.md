# Testability Review

## Verdict: FAIL
The acceptance criteria cover most happy-path scenarios but leave several requirements with no verifiable AC, rely on subjective or ambiguous phrasing, and omit nearly all error/edge-case coverage that the requirements explicitly define.

## Untestable Criteria

1. **"opens a TUI showing all sessions for the current repository"** (AC-1): "opens a TUI" is not a verifiable assertion — it describes appearance, not behavior. A test cannot assert "a TUI was opened." -> Rewrite as: "`koto dashboard` exits with code 0 and renders at least one session row when sessions exist for the current repo; it renders an empty-state message when no sessions exist."

2. **"Sessions are displayed in a hierarchy (root → coordinator → children) with tree connectors"** (AC-2): "displayed in a hierarchy" is visual and subjective. What connectors? Which characters exactly? -> Rewrite as: "In `--once` output, a child session row is prefixed with `├──` or `╰──`; a root session row has no connector prefix; a Level-2 coordinator row has no prefix but its children are indented one level."

3. **"Failed child count in the aggregate row is rendered in red when non-zero"** (AC-6): Color rendering cannot be automatically verified in plain-text or CI test output without terminal emulator support. -> Rewrite as: "In `--once` plain-text output, the failed count field is present and non-zero when failed children exist" (defer color verification to a manual checklist item, or specify an ANSI escape code assertion).

4. **"The focused view shows the current state name matching what `koto status <name>` reports"** (AC-8): This links two commands but doesn't specify the format, field, or exact comparison. It also depends on `koto status` behavior remaining stable. -> Rewrite as: "The `state` field shown in `koto dashboard <name> --once` output equals the `current_state` field returned by `koto status <name> --json`."

5. **"The focused view shows a gate panel with last evaluation result per gate in the current state"** (AC-9): "shows a gate panel" is visual and vague. No assertion about format, fields, or what "last evaluation result" means for never-evaluated gates. -> Rewrite as: "In `--once` output for a focused session, each gate defined in the current state appears with its name, type, and either a pass/fail outcome or `—` if never evaluated in the current epoch."

6. **"A session in a terminal state is shown as `done`"** (AC-11): Does not specify what `done` means in output (text literal, status field, etc.) or how to set up a terminal-state fixture. -> Rewrite as: "In `--once` output, a session whose current state has `terminal: true` in its compiled template is shown with status `done`; a session whose compiled template is missing shows status `unknown`."

7. **"Pressing `q` or `Ctrl+C` exits cleanly with no error output"** (AC-14): "cleanly" is vague; "no error output" doesn't specify stderr vs stdout. Interactive key bindings cannot be tested in standard CI without a PTY harness. -> Rewrite as: "When the process receives SIGINT, it exits with code 0 and produces no output to stderr." (Key binding tests require a documented PTY test approach or manual test matrix.)

8. **"Startup time for `koto dashboard` with 100 sessions is ≤1 second"** (AC-19): Timing assertions in CI are environment-dependent and flaky. The criterion provides no measurement method or acceptable variance. -> Rewrite as: "Measured on the CI host with 100 fixture sessions, `koto dashboard --once` completes in ≤1 second for three consecutive runs (median, not best-of)." Mark this as a performance benchmark test, not a unit test.

## Missing Test Coverage

1. **R3 — Three-level depth limit**: No AC asserts that sessions deeper than 3 levels are rendered flat (no further indentation). A test should verify that a 4-level hierarchy produces the same indentation for Level 3 and Level 4 nodes in `--once` output.

2. **R4 — Status indicator values**: No AC covers the `running`, `failed`, or `blocked` status labels. Only `done` is mentioned (AC-11). A test matrix should cover all four status values with fixture sessions in each state.

3. **R4 — Elapsed time format**: No AC asserts the elapsed time format (`4h 12m`, `45m`, `2s`). A test should verify that each time-range bucket produces the expected human-readable string.

4. **R5 — Aggregate row field mapping**: No AC verifies how `success`, `skipped`, `spawn_failed` child statuses map to the aggregate's `done`/`failed`/`pending` counts. The spec says `skipped` and `success` both contribute to `done`; no test covers this mapping.

5. **R6 — Default expansion for ≤5 children**: AC-3 only tests the `>5` collapsed case. No AC verifies that a coordinator with ≤5 children is expanded by default in `--once` output.

6. **R7 — Within-group name sort**: AC-5 tests priority ordering (failed first, etc.) but no AC verifies the secondary sort by name within each status group.

7. **R8 — Gate display for never-evaluated gates**: No AC covers the `—` outcome for a gate that exists in the current state's template but has no `gate_evaluated` event in the current epoch.

8. **R8 — Gate type display**: No AC verifies that each gate type (`command`, `context-exists`, `context-matches`, `children-complete`) shows the correct key output fields (exit code, `exists` boolean, `matches` boolean, child counts).

9. **R9 — Evidence timeline count cap**: No AC asserts that the timeline shows at most 10 entries when more than 10 `evidence_submitted` events exist.

10. **R9 — Evidence content truncation**: No AC verifies the 80-character truncation of evidence content previews.

11. **R10 — Missing template → `unknown` status**: AC-11 only mentions the happy path (`done`). No AC covers the case where the compiled template is absent, which should yield `unknown` status (specified in R10).

12. **R11 — Epoch-branched sessions hidden from repo-wide list**: AC-12 exists, but no AC verifies the aggregate count in the "Archived epochs" row (AC-13 mentions the row exists, not what it displays).

13. **R12 — `--interval` flag range/validation**: AC-16 tests that `--interval 1000` sets the interval, but no AC covers rejection of invalid values (e.g., `--interval 0`, `--interval -1`, `--interval abc`).

14. **R13 — `--once` exit code**: AC-15 mentions exit code 0, but no AC verifies the format of the plain-text output (columns, separator, order) so testers know what to assert.

15. **R14 — Navigation keys**: The full key binding table (R14) has no AC coverage at all beyond `q` and `Ctrl+C`. `j`/`k`/`g`/`G`/`Enter`/`r`/`?` are untested.

16. **R15 — Unknown event type skipping**: AC-18 covers truncated JSONL, but no AC covers the case where the JSONL contains an unknown `event_type`. Should read without crashing and skip the unknown event.

17. **R17 — Focused view startup time**: R17 requires ≤100ms for a session with ≤1000 events. No AC exists for this performance requirement.

18. **R18 — Update latency**: R18 requires new events to appear within 2× the poll interval. No AC covers this, and no test approach is defined (it requires writing to a fixture JSONL file mid-poll-cycle).

19. **R19 — No async runtime**: No AC verifies the no-async-runtime constraint. Should be a `cargo tree` assertion in CI (verify tokio/async-std do not appear in the dependency graph).

20. **R20 — Single binary**: No AC verifies the binary count. Should be a build artifact assertion.

21. **Error case — `koto dashboard` with no sessions**: No AC covers the empty-repository case. Should render an empty-state message rather than crashing or showing nothing.

22. **Error case — `koto dashboard` invoked outside a git repository**: Session discovery depends on repo-id derivation from cwd. No AC covers what happens when invoked outside a repo.

## Summary

The AC list is 20 items and covers the primary happy-path interactions: launching the TUI, basic session hierarchy, expand/collapse, focused view contents, epoch filtering, `--once` mode, `--interval`, mid-session launch, truncated JSONL, nonexistent session name, and startup time for the repo-wide view. That's a reasonable foundation, but it has three structural gaps. First, 8 of the 20 criteria are phrased in terms of visual appearance or subjective qualities that cannot be mechanically asserted, and only the `--once` flag gives an output surface suitable for automated testing — the criteria don't consistently target that surface. Second, 22 distinct requirements or sub-requirements have no AC at all, including all of R14 (key bindings), R17 (focused view startup time), R18 (update latency), R19 (no async runtime), R20 (single binary), and all secondary sort and field-format behaviors. Third, error and edge cases are almost entirely missing: only the nonexistent session name and truncated JSONL line are covered, while missing templates, invalid interval values, unknown event types, outside-repo invocation, and the empty-repository case have no criteria. A tester working from the AC list alone would produce an incomplete test plan that passes while leaving most of R3–R20 untested.
