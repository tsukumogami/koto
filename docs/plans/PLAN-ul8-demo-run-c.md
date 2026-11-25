---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
milestone: "Build output declaration change on a branch (demonstration run, cleanup on)"
step_count: 1
---

# PLAN: Build output declaration change on a branch (demonstration run, cleanup on)

## Status

Active

This plan is a demonstration of the build-output cleanup that runs at the end
of a single-pr execution, with that cleanup switched on for this repository.
Its branch changes the declared build output, so the run shows that the
cleanup acts on what the merge base declares rather than on the branch's own
declaration. Its pull request isn't meant to merge.

## Scope Summary

Change one value in `.claude/shirabe-extensions/verification-map.json`: the
top-level `build_output` list goes from `["target/"]` to `["demo.local.d/"]`.
Every command, entry, default list, timeout and bound in the map stays as it
is, and nothing else in the repository changes.

## Decomposition Strategy

**Walking skeleton.** The whole change is one step: a single value in one
configuration file, with no code, test or doc touched. There is nothing to
slice further, so step 1 is the entire plan.

## Steps

### Step 1: the verification map declares `demo.local.d/` as its build output

**Goal**: The branch's verification map lists `demo.local.d/` as the
repository's build output in place of `target/`, and nothing else in the map
or the repository changes.

**Acceptance Criteria**:

- [ ] `jq -c .build_output .claude/shirabe-extensions/verification-map.json`
  prints `["demo.local.d/"]`.
- [ ] Apart from `build_output`, the map is unchanged: `jq -S 'del(.build_output)'`
  of the map at the branch's head and at koto's main produce byte-identical
  output.
- [ ] Outside `docs/plans/`, the branch changes only that one line: the diff
  from koto's main to the branch's head shows one other file,
  `.claude/shirabe-extensions/verification-map.json`, with one line removed
  and one added.
- [ ] The verification the repository's verification map selects for this
  change passes.

**Check**: Run `jq -c .build_output .claude/shirabe-extensions/verification-map.json`
and see `["demo.local.d/"]`. Then compare
`git show origin/main:.claude/shirabe-extensions/verification-map.json | jq -S 'del(.build_output)'`
with `jq -S 'del(.build_output)' .claude/shirabe-extensions/verification-map.json`
and see no difference. Then run
`git diff --numstat origin/main...HEAD -- .claude` and see the single line
`1	1	.claude/shirabe-extensions/verification-map.json`, and
`git diff --name-only origin/main...HEAD` list nothing outside `docs/plans/`
and that file. Then see the run's verification step report every selected
command passing.

**Dependencies**: None

**Type**: docs

**Complexity**: simple

**Files**: `.claude/shirabe-extensions/verification-map.json`

## Implementation Sequence

There is one step, and it lands in the plan's single pull request.
