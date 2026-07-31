# Scrutiny: Intent

Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list.
Commit reviewed: b53d61a.

## Design doc alignment (not just literal AC text)

`DESIGN-orphaned-session-detection.md`'s Key Interfaces section gives an exact wire-shape example for
`koto status`:

```json
{
  "stale_template_source_dir": {
    "path": "/home/user/repo-that-was-deleted",
    "machine_id": "host-a",
    "note": "template source directory not found (if this session was synced from another machine, this may be expected)"
  }
}
```

`derive_stale_template_source_dir`'s output matches this shape field-for-field
(`path`/`machine_id`/`note`), and the `note` text for the cloud case is byte-identical to Issue 1's
`format_stale_template_source_note(true)` return value, which the design doc's own example string was
apparently drawn from. This isn't a coincidental match -- Issue 4's whole point per the design's Phase 4
section is wiring the two CLI surfaces to Issue 1's shared wording, not reinventing it, and the
implementation does exactly that.

## Does it serve as a foundation for downstream issues?

Per `PLAN-orphaned-session-detection.md`'s dependency graph, Issue 5 (`koto init`'s collision-path fix,
the actual bug-189 fix) depends only on Issue 1, not on Issue 4 -- the two surfaces are independent
consumers of the same Issue 1 primitives. Issue 4 landing does not block or reshape Issue 5's work, so
there's no foundation concern to check beyond "did this leave Issue 1's shared surface intact for Issue
5 to also consume" -- it did: `check_template_source_dir` and `format_stale_template_source_note`'s
signatures are unchanged by this commit (`git diff` touches only `src/cli/mod.rs`, `src/cli/session.rs`,
`docs/guides/cli-usage.md`, plus the new test file).

## Behavioral correctness beyond the AC checklist

- **Backend-aware gating is per-call, not per-struct.** `handle_status` computes `is_cloud` once from its
  own `backend` parameter; `handle_list` computes `is_cloud` once from its own `backend` parameter. Both
  are the CLI-invocation's actual backend (`build_backend()` in `src/cli/mod.rs`), not a stale or
  per-session value -- correct, since `is_cloud` genuinely is a property of how the *current process* is
  configured to talk to a session's storage, not a property recorded on the session itself.
- **`handle_list`'s per-row mutation only touches rows that already have a non-null
  `template_source_status`.** The `row.get("template_source_status").and_then(|s| s.get("exists")) ==
  Some(&Value::Bool(false))` check short-circuits to `false` (no mutation) when the field is `null` --
  correctly handling both the "no `template_source_dir` recorded" and the `CloudBackend` remote-only
  placeholder-row cases from Issue 3's doc comment, without needing to special-case either explicitly.
- **No new I/O introduced.** `handle_list` performs zero additional filesystem or network calls beyond
  the existing `backend.list()` -- confirmed by reading the diff; the only new work is JSON
  serialization/mutation of data already in memory. This matches the design's explicit "no new I/O at
  this layer" statement for `handle_list`.

## Test realism

The new integration tests (`tests/stale_template_source_dir_cli_test.rs`) exercise the real `koto`
binary end to end, including a genuine `Backend::Cloud` constructed via `koto config set` pointed at an
RFC 5737 non-routable endpoint -- not just a mocked `is_cloud: bool` parameter. This means the tests
actually exercise `Backend::is_cloud()`'s `matches!(self, Backend::Cloud(_))` branch and
`CloudBackend::list()`/`read_header()`'s real (fail-fast, non-fatal) S3-failure handling, which is a
stronger check of intent than only unit-testing the pure wording function would have been.

## Verdict

blocking_count: 0
advisory_count: 0

The implementation matches the design doc's described wire shape exactly, preserves Issue 1's shared
primitives unchanged for Issue 5, and its tests exercise real backend behavior rather than only the
literal per-function contract.
