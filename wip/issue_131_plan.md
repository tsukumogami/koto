# Plan — Issue #131: --with-data @filepath

## Problem

`koto decisions record --with-data @file` and `koto overrides record --with-data @file` fail with `invalid JSON`: both handlers parse the raw argument directly instead of routing through `resolve_with_data_source`. Only `handle_next` (PR #130) plumbed the helper.

The bug reporter described the failure against `koto next`, but that path actually works. The defect is in the sibling subcommands; the user likely encountered it through downstream tooling (release flow) that uses `decisions record` or `overrides record`.

## Approach

1. **Reproduce with tests first** (explicit user ask). Add integration tests covering both subcommands that read `@file.json` and verify payloads are parsed correctly. These tests must fail on main.
2. **Route through the shared helper.** Call `resolve_with_data_source` at the top of both handlers, before the size check and JSON parse.
3. **Inline JSON still works** — `resolve_with_data_source` returns the input unchanged when there's no `@` prefix.
4. **Preserve error-envelope shape.** On resolution failure, surface the same `{"command": "<subcommand>", "error": "..."}` envelope the handlers already produce for other errors. `resolve_with_data_source` returns `NextError` with `InvalidSubmission` code; map the message into the existing envelope rather than propagating the `koto next` shape.

## Files

- `src/cli/mod.rs` — `handle_decisions_record`
- `src/cli/overrides.rs` — `handle_overrides_record`
- `tests/decisions_with_data_at_file_test.rs` — new integration test
- `tests/overrides_with_data_at_file_test.rs` — new integration test

## Risks

- Error-envelope drift between the three subcommands (`next` uses typed `NextError` JSON; `decisions`/`overrides` use flat `{command, error}`). Mitigation: extract the message from the resolver error and fold it into the existing envelope instead of propagating the typed shape.
- Oversize-file check: `resolve_with_data_source` already enforces `MAX_WITH_DATA_BYTES` by file size; the existing post-read length check becomes redundant but harmless — leave it for defense-in-depth.

## Testing Strategy

- Happy path: `@file` reads valid JSON, payload accepted.
- Missing file: `@/does/not/exist` → clear "cannot read file" error.
- Empty path: `@` alone → "requires a file path after '@'".
- Inline JSON still works unchanged (regression).

No docs need updating — PR #130's "Evidence ergonomics" release-note bullet says `--with-data @file.json reads submissions from disk with a 1 MB cap`, which is now accurate across all subcommands (it was advertised but only half-implemented).
