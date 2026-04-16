# Baseline

## Environment
- Date: 2026-04-15
- Branch: fix/131-with-data-at-file-other-subcommands
- Base commit: f03c531

## Bug Reproduction

`koto decisions record --with-data @<file>` and `koto overrides record --with-data @<file>` both fail with `invalid JSON in --with-data: expected value at line 1 column 1` — the `@` prefix is not stripped and file contents are not read.

`koto next --with-data @<file>` works because `handle_next` routes the raw string through `resolve_with_data_source`. The sibling handlers do not.

## Build Status
cargo build --release: pass (34s).

## Pre-existing Issues
None related to this work.
