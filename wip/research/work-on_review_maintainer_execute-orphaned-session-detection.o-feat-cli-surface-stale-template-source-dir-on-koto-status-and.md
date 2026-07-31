# Review: Maintainer

Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list.
Commit reviewed: b53d61a.

## Naming

- `derive_stale_template_source_dir` follows the existing `derive_batch_view`/`derive_superseded_branches`
  naming convention in the same function -- a future reader scanning `handle_status` sees three
  `derive_*`-named helpers feeding three conditional response keys and can infer the pattern immediately.
- Test names (`status_omits_stale_key_when_template_source_dir_still_exists`,
  `list_surfaces_softened_wording_for_cloud_backend`, etc.) describe both the surface and the exact
  condition under test -- readable as a spec without opening the test bodies.

## Implicit contracts made explicit

- The doc comment on `handle_status`/`handle_list`'s new `&Backend` parameter states *why* the signature
  changed (needs `Backend::is_cloud()`, mirrors `handle_resolve`) rather than leaving a future reader to
  reconstruct the reasoning from a diff or commit message alone.
- `derive_stale_template_source_dir`'s doc comment states the "never serialize `null`, omit the key
  instead" contract explicitly, matching the existing `stale_template_source_dir` present-only-when-relevant
  wording documented in the design.

## One advisory: stringly-typed field access in `handle_list`

`handle_list`'s row-mutation logic reads `row.get("template_source_status").and_then(|s|
s.get("exists"))` and writes `status["note"] = ...` using string literal JSON keys rather than typed
struct field access. If `TemplateSourceStatus` or `SessionInfo`'s field names ever change, this code
would silently stop matching (no compiler error) rather than fail to build. This is not a new pattern in
this file or file family -- `handle_status`'s existing `response["batch"]` / `response["superseded_branches"]`
assignments a few lines above use the identical stringly-typed idiom for the same
present-only-when-relevant JSON shaping -- so this is consistent with, not a departure from, the
established local style. Flagged as advisory for a future maintainer's awareness (e.g. a possible follow-up
would be a small compile-time-checked constant for `"template_source_status"`/`"exists"`/`"note"` shared
across both call sites), not because it's wrong for this commit to follow the existing convention.

## Context clarity

Both changed functions retain and extend their existing doc comments rather than replacing them, so the
new behavior reads as an addition to documented behavior, not an undocumented side effect discoverable
only by reading the body.

## Verdict

blocking_count: 0
advisory_count: 1 (stringly-typed JSON key access in `handle_list`, consistent with existing local
convention -- see above)

A future developer extending either surface (e.g. adding a third conditional key) has a clear pattern to
follow, with the one advisory being about a pre-existing codebase idiom this commit continues rather than
originates.
