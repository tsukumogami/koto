# Scrutiny: Intent

Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's
already-exists error. Commit reviewed: e35f42b.

## Design intent vs. implementation

**The core bug (tsukumogami/koto#189).** The design's Problem Statement is
explicit: a session whose `template_source_dir` working tree was torn down
looks, from `koto init`'s perspective, permanently "alive" -- re-running
`koto init <same-name>` produces a generic "already exists" error
indistinguishable from a real concurrent collision. The implementation
directly closes this gap: the new
`init_collision_diagnoses_stale_template_source_dir` integration test
reproduces the exact scenario (init with a real template dir, delete it,
re-init) and confirms the error now names the missing path. This is the
actual fix, not just AC-satisfying scaffolding -- the repro test fails
without this commit (verified by reading `stale_template_source_dir_clause`:
before this commit, the pre-check never called `read_header` at all, so
there was no path to inject the clause).

**"Same clause, different base messages" -- matches the design's Implicit
Decision, not a superficial reading of the AC.** The design explicitly
corrects an earlier draft's claim that the two base messages were
byte-identical (they aren't -- pre-check keeps cleanup guidance, Collision
handler is shorter) and concludes both paths should still get the *same
staleness clause* appended, "not... unify the two base messages themselves."
The implementation follows this precisely: `base` strings are untouched
verbatim from the pre-commit code at both sites, and only the appended
`Some(clause)` branch is shared logic. A shallower reading of the AC could
have "fixed" the perceived inconsistency by unifying the base messages
outright (simpler code, arguably "more consistent") -- that would have
contradicted the design's explicit reasoning and silently changed
externally-visible behavior (`init_duplicate_error_mentions_remediation`
depends on the pre-check's cleanup guidance existing). The implementation
did not take that shortcut.

**Foundation for downstream work.** This is the last issue in the plan
(no downstream issues in this design), so there is no forward-compatibility
concern to check beyond "does this leave the codebase in a state a future
maintainer can build on." `stale_template_source_dir_clause` is a small,
single-purpose, well-documented private function scoped to `koto init`'s
collision paths -- it does not attempt to become a fourth general-purpose
consumer of `TemplateSourceStatus` beyond what Issue 1's module already
offers (`check_template_source_dir`,
`format_stale_template_source_note`), matching the "one computation, thin
purpose-specific projections" pattern the design established across
Issues 1/2/4. No new public API surface, no new JSON wire format (the
`error`/`command` envelope shape is unchanged, only the `error` string's
content grows conditionally).

**One place where the implementation is more conservative than the design's
literal words might suggest, correctly so.** The design's Data Flow section
(step 2) says the three new surfaces "each load the relevant header through
the existing shared parser (`persistence::parse_header`)." The
implementation instead uses `backend.read_header(name)` (the
`SessionBackend` trait method), not `persistence::parse_header` directly.
Checked `src/session/local.rs`/`cloud.rs`: `read_header` is itself
implemented in terms of the shared header-parsing logic and is the same
method `handle_status` (Issue 4) already uses for this exact purpose (see
`src/cli/mod.rs`'s `handle_status`, which calls `backend.read_events`, not
`persistence::parse_header` directly, either) -- so this issue's
implementation is consistent with the precedent Issue 4 already set, and the
design's phrasing was describing the effect (get a header via the existing
shared machinery) rather than mandating the exact function name. Not a
deviation from intent.

## Verdict

`blocking_count: 0`, `advisory_count: 0`. The implementation is the actual
fix for the reported bug, correctly preserves the design's deliberate
base-message divergence, and stays within the established module boundaries
without inventing new abstractions this issue doesn't need.
