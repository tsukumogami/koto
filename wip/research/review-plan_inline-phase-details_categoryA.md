# Category A: Scope Gate

## Verdict
pass

## Critical findings
None.

## Reasoning

**Issue count vs. design scope.** The design names 7 rows in its Solution
Architecture table (event model, delivery predicate, response combinator,
pointer splice, natural path, directed path, retrieval) and 4 implementation
phases. The plan produces 5 issues. Against the heuristic range (too few:
< half the component count; too many: > 5x), 5 is well inside bounds against
either the 7-row or 4-phase baseline. Walking the component table against the
issue set: every row lands in an issue's acceptance criteria (types.rs ->
Issue 1; persistence.rs -> Issue 1; next_types.rs combinator -> Issue 2;
next_types.rs pointer -> Issue 4; mod.rs natural path -> Issue 2; mod.rs
directed path -> Issue 2; handle_status -> Issue 3). No component is
unaddressed.

**Five issues for four design phases -- right-sized, not fragmented.** The
plan follows the design's own four-phase sequence and splits only the
design's Phase 4 ("the pointer, then the documentation and evals") into two
issues: Issue 4 (pointer) and Issue 5 (docs/evals). The design itself flags
these as separable ("Phases 3 and 4 could be separated, but phase 4 without
phase 3 points at nothing" -- and by extension, the pointer and the
documentation of it are different kinds of work with different completion
criteria: the pointer is testable code, the docs/evals item is prose and
eval-file work gated on the shipped behavior being final). Splitting them is
a legitimate atomicity refinement, not fragmentation. No pair in the plan
warrants merging: each issue outline has a distinct, checkable deliverable
and a stated reason it can't fold into its neighbor without losing that
checkability.

**Issue 2 is large but is one atomic unit, not a bundle wearing one name.**
Its AC list is the plan's longest because it enumerates every arrival path
(gate-blocked non-advance, loop-back, self-transition, unconditional
transition, rewind, init, directed transition, two directed transitions in a
row) that the *same* single rule change must hold across -- these are test
scenarios over one behavior change, not multiple independent deliverables.
Critically, the design states a hard coupling explicitly: "phase 2's two
halves, wiring the natural path and wiring the directed path, must land
together: shipping one without the other leaves the two paths disagreeing,
which is the defect R4 exists to close." Splitting Issue 2 by call site would
produce an intermediate state where the two paths disagree -- exactly the
defect this work exists to fix -- so the size is dictated by the design's own
correctness constraint, not by decomposition laziness. This also correctly
earns the plan's only `critical` complexity rating, which is the
architecturally significant piece the design's four decisions converge on.

**Issue 1 is not too small despite being inert by construction.** The design
says so explicitly: "Phase 1 is inert by construction... so it can land on
its own." It has a real, non-trivial test surface (six unit-test scenarios
over synthetic event lists, a schema-version pin, and a backward-compat check
for older binaries) and is a genuine prerequisite Issue 2 depends on. Since
execution mode is single-pr, "too small to be its own unit" doesn't carry the
multi-PR-overhead cost the atomicity guard is protecting against here -- it's
a commit-grouping boundary inside one PR, and the decomposition rationale
(predicate must exist before either call site can consume it) is sound.

**Single-pr execution mode holds up against the actual split-trigger
branches.** Checked the decomposition notes' Phase 3.6 reasoning against
`references/split-triggers.md` directly rather than taking the notes at face
value:
- Hard Constraint requires a named, non-optional fact -- cross-repo work with
  load-bearing landing order, a workflow file that must reach the default
  branch before invocation, a step whose output must be published/deployed/
  merged before a later step consumes it, or a merge gate between steps. None
  apply: single Rust repo, no publish/deploy step, no merge gate between the
  five issues (they're sequential commits inside one review, not separately
  landed PRs).
- Incremental Value: the 3.5a guard correctly found no unit is a standalone
  increment (Issue 2 fixes nothing usable until Issue 3/4 give a recovery
  path; Issue 1 is inert; Issue 5 documents behavior that doesn't exist until
  the rest lands). This matches the design's own framing of the change as one
  organizing idea composed from four decisions, not four independent
  deliverables.
- Stated Preference: koto's CLAUDE.md carries no `## Delivery Preference:`
  header declaring `atomic`, so this branch doesn't fire either.

No branch fires, so single-pr with no `split_rationale` is the correct
outcome, not a shortcut the plan took.

**Docs-coverage backstop.** The design's frontmatter carries no
`user_visible_surface` field, and its body contains no `docs/guides/*`
reference (confirmed by grep -- zero hits), so per the detection contract
the backstop does not trigger for this design input. The plan includes docs
coverage anyway (Issue 5, matching PRD R20-R25's downstream obligations,
including `docs/guides/cli-usage.md`), so this is moot regardless -- no gap
either way.

**No scope creep found.** Checked the plan against the PRD's Out of Scope
list (auto-advance discarding phases, two consecutive rewinds moving
forward, `accepts:` not gating advancement, migration scan output, template
retrofitting, changing `derive_visit_counts` semantics) and the PRD's own
added exclusion (directed path evaluating gates). None appear in any issue.
Issue 2's AC explicitly pins `derive_visit_counts` as untouched, which is a
direct guard against the one exclusion a decomposition could most easily
drift into by accident. Issue 3's template-hash-verification AC is not
creep -- it implements the design's explicit Security Considerations ruling
("`handle_status` verifies the hash, and reports a mismatch rather than
failing on it"), not an addition beyond the design.
