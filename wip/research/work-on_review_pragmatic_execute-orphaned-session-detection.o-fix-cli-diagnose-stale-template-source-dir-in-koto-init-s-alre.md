# Review: Pragmatic

Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's
already-exists error. Commit reviewed: e35f42b.

## Simplicity

The production-code diff is small and proportionate to the fix: one new
~30-line private function (`stale_template_source_dir_clause`) plus two
~9-line call-site edits that each replace a `format!(...)` literal with a
`base` + optional-`clause` two-line pattern. No new public API, no new
struct, no new module. The function does exactly one thing (best-effort
compute an optional string to append) and both callers use it identically.

No dead code: every branch of `stale_template_source_dir_clause` (header
unreadable, no recorded dir, dir exists, dir missing) is exercised by a
dedicated unit test, and both call sites are exercised end-to-end by
integration tests. No unused imports, no leftover TODOs.

No scope creep: `handle_init_inline` is deliberately left untouched
(see Justification review for why extending it would have been dead code
anyway -- the inline path never records `template_source_dir`). The
function signature change (`&dyn SessionBackend` -> `&Backend` on
`handle_init`) is the minimum required to reach `Backend::is_cloud()`, and
mirrors the exact precedent Issue 4 already set for `handle_status` -- not
a new pattern introduced by this issue.

## Minor observation (advisory)

The clause format string is `" ({}: {})"` -- a leading space, parens, the
note, a colon, then the path. This produces output like:

```
workflow 'sess' already exists; ... (template source directory no longer exists: /tmp/.../srctpl)
```

This is readable and grep-able (both the note text and the path are
substring-testable, which the tests rely on), but it's the only new
"format grammar" introduced in this diff and isn't reused anywhere else, so
there's no established convention it either follows or breaks -- it's a
one-off judgment call. Not a simplicity problem, just noting it as the one
piece of genuinely new formatting logic in an otherwise mechanical diff.

## Verdict

`blocking_count: 0`, `advisory_count: 1` (clause format string is a
one-off, undocumented-elsewhere convention; low stakes, purely cosmetic).
