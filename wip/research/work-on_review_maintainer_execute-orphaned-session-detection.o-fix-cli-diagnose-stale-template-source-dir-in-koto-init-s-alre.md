# Review: Maintainer

Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's
already-exists error. Commit reviewed: e35f42b.

## Readability and naming

`stale_template_source_dir_clause` follows the existing
`stale_template_source_dir`/`derive_stale_template_source_dir` vocabulary
already established in this file by Issue 4 -- a maintainer who has seen one
will immediately recognize the family the other belongs to. The `_clause`
suffix is a clear, accurate description of what it returns (a fragment to
append, not a full message or a JSON value), distinguishing it from
`derive_stale_template_source_dir`'s JSON-value return without needing to
read the body.

The doc comment on `stale_template_source_dir_clause` is thorough: it names
both call sites explicitly, explains *why* a single shared function
guarantees the "identical clause" AC (rather than just asserting it), and
enumerates every `None` case with the reasoning behind each ("must never
crash or replace the existing... error with a different one" for the
header-read failure case). A future maintainer changing this function does
not need to re-derive the contract from the call sites -- it's stated
up front.

Both call sites carry a short comment explaining why the `base` variable is
now separate from the `error` variable and why the `Collision` handler
specifically calls out that it shares the pre-check's helper -- readable
without cross-referencing the design doc.

## Implicit contracts made explicit

The most important implicit contract in this change -- "the two collision
paths must never see byte-different clauses for the same underlying
condition, but their base messages are legitimately different" -- is easy to
accidentally break in a future edit (e.g., a future author "simplifying" by
inlining slightly different formatting at each site). The doc comment on
`stale_template_source_dir_clause` states this explicitly as the reason the
function exists as a single shared unit rather than duplicated inline logic
at each call site, and the
`stale_template_source_dir_clause_identical_across_both_collision_paths`
unit test's own comment restates the same contract at the point a future
maintainer would look to verify a change didn't break it. This is exactly
the kind of "why," not just "what," documentation this project values.

## Test readability

Test names are self-describing
(`stale_template_source_dir_clause_none_when_header_unreadable`,
`init_collision_diagnoses_stale_template_source_dir`) and each test's body
follows the file's existing arrange/act/assert shape without introducing a
new testing idiom. The integration test's module-level doc comment was
updated to explain why the `SpawnErrorKind::Collision` path isn't covered
here (pointing a future reader at the unit tests instead of leaving them to
wonder why only the pre-check is exercised end-to-end).

## Verdict

`blocking_count: 0`, `advisory_count: 0`. Naming, doc comments, and test
names are consistent with the file's established conventions and make the
non-obvious "same clause, different base message" contract explicit rather
than something a future maintainer has to rediscover.
