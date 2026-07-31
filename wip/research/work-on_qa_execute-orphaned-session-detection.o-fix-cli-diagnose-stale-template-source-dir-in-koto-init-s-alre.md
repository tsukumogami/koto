# QA: Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's already-exists error

Commit tested: e35f42b. Debug build (`cargo build`), exercised the real
`target/debug/koto` binary end to end from the user's perspective (fresh temp
`$HOME`/`$KOTO_SESSIONS_BASE` per scenario), independent of the automated
test suite.

## Scenario 1: normal init succeeds unaffected

`koto init sess1 --template <real template>` -> `{"name":"sess1","state":"start"}`,
exit 0. **Pass.** No behavior change on the non-collision path (confirms the
new I/O in the pre-check is genuinely gated on `backend.exists(name)` being
true, not always paid).

## Scenario 2: collision without staleness -- message unchanged

Re-ran `koto init sess1 ...` immediately (template dir still present).
Output: `"workflow 'sess1' already exists; run \`koto session cleanup sess1\`
to reuse the name, or \`koto cancel --cleanup sess1\` to stop a running
workflow first"` -- byte-identical to the pre-commit message, no clause
appended. **Pass.**

## Scenario 3: collision with stale template_source_dir -- the bug repro

Inited `sess2`, deleted the template's parent directory, re-ran `koto init
sess2 ...`. Output:

```
workflow 'sess2' already exists; run `koto session cleanup sess2` to reuse
the name, or `koto cancel --cleanup sess2` to stop a running workflow first
(template source directory no longer exists: /tmp/tmp.RUCIBKtvKX/tpl)
```

The base message is untouched and the clause names both the staleness
condition and the exact missing path. **Pass.** This is a live, manual
confirmation of the tsukumogami/koto#189 fix, independent of the automated
`init_collision_diagnoses_stale_template_source_dir` integration test.

## Scenario 4: corrupt state file -- best-effort, no crash, no different error

Inited `sess3`, then overwrote its on-disk state file
(`koto-sess3.state.jsonl`) with garbage (`not valid jsonl at all {{{`), then
re-ran `koto init sess3 ...`. Output: the unchanged base "already exists"
message, no staleness clause, exit code 1 (verified separately: not 0, not a
panic, not a different error type). **Pass.** Confirms the best-effort
`read_header(...).ok()?` contract holds against real on-disk corruption, not
just the constructed-in-memory `Err` case the unit test exercises.

## Automated suite cross-check

Also re-ran the full automated suite as a sanity cross-check (not a
substitute for the manual scenarios above): `cargo test` -- all binaries
pass, 0 failures, including all 9 tests in
`tests/stale_template_source_dir_cli_test.rs` (3 new `init_collision_*`) and
5 new unit tests in `src/cli/mod.rs`. `cargo fmt --check` and `cargo clippy
--lib -- -D warnings` both clean.

## Verdict

4/4 manual scenarios passed. No defects found from a user-facing
perspective; the fix behaves as designed under both the happy path and a
real (not simulated) corruption case.
