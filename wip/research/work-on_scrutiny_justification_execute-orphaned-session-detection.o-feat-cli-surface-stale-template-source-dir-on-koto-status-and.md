# Scrutiny: Justification

Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list.
Commit reviewed: b53d61a.

## Deviations from the plan and their justification

### 1. `handle_status`/`handle_list` signature change: `&dyn SessionBackend` -> `&Backend`

The plan/AC text doesn't spell out this signature change explicitly, but it's a direct consequence of a
tension the design doc already calls out by name (Decision 2's "Backend differentiation" discussion,
`docs/designs/DESIGN-orphaned-session-detection.md` lines 577-590): `Backend::is_cloud()` is an inherent
method on the concrete `Backend` enum, not part of the `SessionBackend` trait both functions previously
took as `&dyn SessionBackend`. Two options existed: widen the trait object interface with a
concrete-enum-only accessor, or change the two call sites to the concrete type. `handle_resolve`
(`src/cli/session.rs:525`) already takes `&Backend` for exactly this reason (it needs `Backend::Cloud`-specific
branching), so following that precedent rather than growing the trait is consistent with the codebase's
existing answer to the same problem, not a new pattern introduced ad hoc. This is a genuine trade-off,
not a shortcut: the alternative (trait method) would leak backend-concrete concerns into every
`SessionBackend` implementor, including test doubles, for a single accessor two call sites need.

### 2. `derive_stale_template_source_dir` as a private pure helper

Not mentioned in the AC text, but mirrors the existing `derive_batch_view`/`derive_superseded_branches`
functions immediately above/below it in `handle_status`, both of which already follow the same
present-only-when-relevant pattern this issue extends. Keeping the computation in a testable pure
function (header + bool in, `Option<Value>` out) rather than inlining it in `handle_status` is what makes
the omitted/direct/softened distinction unit-testable without capturing `println!` output -- a real
testability benefit, not just style preference.

### 3. `handle_list`'s JSON-`Value` mutation instead of a new field on `TemplateSourceStatus`

Considered and rejected: adding a `note: Option<&'static str>` field directly onto `TemplateSourceStatus`
(Issue 1's struct, shared with the scheduler warning path and `SessionInfo`). Rejected because
`TemplateSourceStatus` is computed backend-agnostically by `check_template_source_path`/
`check_template_source_dir` (Issue 1), which have no `is_cloud` parameter and are frozen by that issue's
"nothing outside this new module calls any of the three new items yet" framing -- baking wording into the
struct would require plumbing `is_cloud` through the scheduler's per-tick probe too, which explicitly does
not need or want wording (`emit_template_source_dir_warnings` builds its own warning shape). Converting
`Vec<SessionInfo>` to `serde_json::Value` and mutating the row in place keeps wording a CLI-presentation
concern, computed once per `handle_list` call from that call's own backend, without touching the shared
struct's contract. This is more verbose than a struct field would be, but it's the correct layering given
Issue 1 already shipped and its module doc explicitly says "purely additive... does not wire it into any
existing call site" -- Issue 4 shouldn't retroactively widen Issue 1's struct to serve a single new
consumer's wording need.

### 4. `docs/guides/cli-usage.md`'s `#### session list` JSON example not literally extended

The AC says the section "gets the new field noted." I added a prose paragraph describing
`template_source_status`'s shape and the conditional `note` field rather than editing the JSON example
block itself. Justification: the existing example already omits a real, currently-serialized field
(`parent_workflow`, present on every `SessionInfo` since before this issue) -- the doc's own convention
for this section is an illustrative, non-exhaustive example plus prose describing the full field set, not
a literal schema dump. Extending the JSON block would have meant either padding it with `parent_workflow`
too (out of scope for this issue) or introducing an inconsistency where only the newest field appears in
the example. This is flagged as advisory in the completeness review rather than treated as silently
resolved, since a reviewer could reasonably prefer the example be extended -- it's a judgment call, not an
oversight.

## Verdict

blocking_count: 0
advisory_count: 1 (doc JSON-example-vs-prose choice, see #4 above -- surfaced for a maintainer's call, not
blocking)

All deviations reflect real trade-offs traceable to either existing codebase precedent
(`handle_resolve`'s `&Backend`) or Issue 1's own stated scope boundary (`TemplateSourceStatus` staying
backend-agnostic), not shortcuts taken to avoid harder work.
