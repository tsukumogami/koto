# Plan Decisions: orphaned-session-detection

## Round 1

- **Decomposition strategy: horizontal, not walking skeleton.** The design
  refactors existing code and extends an already-working check layer by
  layer; no new end-to-end user flow needs early stub-and-refine
  validation. `confirmed` -- clear fit, no ambiguity.
- **Value confirmation (step 3.5a): pass by construction.** Single-pr mode
  means one unit (the whole plan); the "standalone increment" question
  only applies when a plan is split across multiple PRs. `confirmed`.
- **Execution mode: single-pr.** Set directly by explicit user instruction
  ("make it a single-pr plan"), not derived from the surfaced rule's
  default heuristic. `confirmed` -- no ambiguity, no override needed.
- **Docs-coverage emit: folded into Issue 4's acceptance criteria**, not a
  dedicated docs issue. The user-visible surface (two new JSON fields) is
  small enough to document alongside the issue that introduces it, rather
  than warranting its own issue. `confirmed`.

## Round 1: Phase 6 review findings and resolution

`/review-plan` fast-path found: Category A clean; Category B two findings
(`Backend::is_cloud()` design-vs-plan contradiction affecting Issues 4/5;
`path_resolution.rs` architecture mismatch between two design sections and
the actual call graph, affecting Issues 1/2); Category C two findings
(Issue 1 happy-path-only, no edge-case AC; same `is_cloud()` contradiction
via pattern 6); Category D one finding (Issue 5's dependency graph doesn't
force ordering relative to Issue 4's `is_cloud()` helper).

Rather than the mechanical loop-back-to-Phase-1 (Category B's nominal
`loop_target`), which would wipe milestones/decomposition that aren't
actually wrong, this was resolved narrowly and directly, `status="assumed"`
(no interactive user available):

- **`Backend::is_cloud()` settled as a real accessor**, added once in
  Issue 1 (not Issues 4 or 5 independently) as foundational shared
  infrastructure both later issues already depend on. Resolves the B and C
  contradiction and the D missing-dependency-edge risk in one move, without
  adding a new dependency edge between Issues 4 and 5.
- **`path_resolution.rs` confirmed and documented as explicitly out of
  scope** for this whole plan -- verified directly against
  `resolve_template_path_with_base_status`'s actual signature
  (`base_exists: Option<bool>`, no path/header in its own scope) and its
  two callers in `batch.rs`. The design doc and Issue 2 are corrected to
  describe only `batch.rs`'s single existence probe and
  `emit_template_source_dir_warnings` as touched.
- **Issue 1 gains one edge-case unit test AC** beyond the three happy-path
  cases (existence/absence), per Category C's pattern-3 finding.

Design doc, Issues 1/2/4/5 bodies, and this decomposition's outlines were
corrected directly rather than regenerated from scratch via fresh Phase 4
agents, since the root causes were narrow and precisely understood after
verifying each claim against the actual source. `review_rounds`
incremented to 1 in the analysis artifact.

## Round 2: re-review findings and resolution

A full re-run of all four categories against the round-1 fixes found
Categories A and B clean, but two residual findings:

- **Category D**: round 1 only moved `Backend::is_cloud()` into Issue 1;
  Issue 5's AC still hedged on reusing Issue 4's *wording-formatting
  helper* ("if it has already landed... or an equivalent local check
  otherwise") with no dependency edge forcing order. Same class of gap as
  round 1, one layer up. **Fixed** by moving
  `format_stale_template_source_note` itself into Issue 1 (and the design
  doc's Solution Architecture / Phase 4) alongside the accessor -- Issue 4
  now only consumes it, matching Issue 5.
- **Category C**: Issue 1's round-1 "boundary case" AC didn't trip the
  taxonomy's Pattern-3 detector because it used "boundary case" instead of
  the taxonomy's literal trigger phrase "edge case" -- a wording miss, not
  a substance gap. **Fixed** by renaming to "edge case" and splitting into
  two explicit ACs: a genuine edge case (dangling symlink) and a genuine
  error/invalid-input case (path resolves to a regular file, not a
  directory), both asserting specific, checkable behavior rather than a
  generic label.

`status="assumed"` for both fixes (no interactive user available). A
third re-review pass was launched to confirm both are resolved before
finalizing.
