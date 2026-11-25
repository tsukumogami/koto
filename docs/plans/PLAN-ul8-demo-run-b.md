---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
milestone: "Session retention guide wording fix (demonstration run, cleanup on)"
step_count: 1
---

# PLAN: Session retention guide wording fix (demonstration run, cleanup on)

## Status

Active

This plan is a demonstration of the build-output cleanup that runs at the end
of a single-pr execution, with that cleanup switched on for this repository.
Its pull request isn't meant to merge.

## Scope Summary

Fix one sentence in `docs/guides/session-retention.md`: line 24 is missing
the word "that", so it reads "A session `koto session import` moved is kept
too". The fix adds the word and changes nothing else in the repository.

## Decomposition Strategy

**Walking skeleton.** The whole change is one step: a single-line wording
fix in one guide, with no code, test or other doc touched. There is nothing
to slice further, so step 1 is the entire plan.

## Steps

### Step 1: the session retention guide's import sentence reads correctly

**Goal**: A reader of the session retention guide sees the sentence about
imported sessions with its missing word restored, and nothing else in the
repository changes.

**Acceptance Criteria**:

- [ ] Line 24 of `docs/guides/session-retention.md` reads exactly
  ``A session that `koto session import` moved is kept too, by the copy. The import``.
- [ ] Outside `docs/plans/`, the branch changes only that one line: the diff
  from koto's main to the branch's head shows one other file,
  `docs/guides/session-retention.md`, with one line removed and one added.
- [ ] The verification the repository's verification map selects for this
  change passes.

**Check**: Run `sed -n 24p docs/guides/session-retention.md` and see
``A session that `koto session import` moved is kept too, by the copy. The import``.
Then run `git diff --numstat origin/main...HEAD -- docs/guides` and see the
single line `1	1	docs/guides/session-retention.md`, and
`git diff --name-only origin/main...HEAD` list nothing outside `docs/plans/`
and `docs/guides/session-retention.md`. Then see the run's verification step
report every selected command passing.

**Dependencies**: None

**Type**: docs

**Complexity**: simple

**Files**: `docs/guides/session-retention.md`

## Implementation Sequence

There is one step, and it lands in the plan's single pull request.
