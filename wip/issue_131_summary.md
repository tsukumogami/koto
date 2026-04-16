# Summary — Issue #131: `--with-data @filepath`

## What Was Implemented

Routed `--with-data` through the `@file` resolver for `koto decisions record` and `koto overrides record`. The feature was advertised as working across subcommands (and implemented for `koto next` in PR #130) but the sibling handlers bypassed the resolver, causing `@file.json` to fail with a misleading `invalid JSON` error.

## Changes Made

- `src/cli/mod.rs`: `handle_decisions_record` now calls `resolve_with_data_source` before parsing.
- `src/cli/overrides.rs`: `handle_overrides_record` does the same for its optional `with_data` argument.
- `tests/integration_test.rs`: 4 new integration tests (happy path + missing-file for both subcommands). Written first to reproduce the bug, then used to validate the fix.
- `plugins/koto-skills/skills/koto-user/references/command-reference.md`: documents `@file` support on both subcommands.

## Key Decisions

- Preserved the existing flat `{command, error}` response envelope for `decisions`/`overrides` instead of switching to the typed `NextError` JSON shape used by `koto next`. Keeps these subcommands consistent with their prior error shape and avoids a breaking change to consumers.
- The existing post-resolve payload size check in `decisions record` becomes redundant (resolver already enforces `MAX_WITH_DATA_BYTES` by file size). Left it for defense-in-depth against inline JSON that exceeds the cap — no behavior change.

## Test Coverage

- New tests: 4
- Full suite: 1040 tests, all pass (818 unit + 171 integration + 51 other test binaries).
- Bug reproduction confirmed: all 4 tests fail on `main` before the fix and pass after.

## Known Limitations

None.

## Requirements Mapping

| AC | Status | Evidence |
|----|--------|----------|
| `--with-data @file` reads file contents | Implemented | `decisions_record_with_data_at_file_reads_decision_from_file`, `overrides_record_with_data_at_file_reads_value_from_file` |
| Missing file surfaces clear error naming the path | Implemented | `*_missing_returns_clear_error` tests |
| Inline JSON still works | Implemented (regression) | Existing `gate_override_full_flow` and other tests continue to pass |
