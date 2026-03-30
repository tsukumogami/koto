---
name: code-review
version: "1.0"
description: Route a code review through approval, revision, or deferral based on reviewer verdict
initial_state: review

variables:
  PR_NUMBER:
    description: Pull request number to review
    required: true

states:
  review:
    accepts:
      verdict:
        type: enum
        values: [approve, request-changes, defer]
        required: true
    transitions:
      - target: merge_prep
        when:
          verdict: approve
      - target: revision
        when:
          verdict: request-changes
      - target: parked
        when:
          verdict: defer
  revision:
    accepts:
      status:
        type: enum
        values: [revised]
        required: true
    transitions:
      - target: review
        when:
          status: revised
  merge_prep:
    terminal: true
  parked:
    terminal: true
---

## review

Review pull request #{{PR_NUMBER}}.

Read the diff, check for correctness, style, and test coverage. When you've formed a judgment, submit your verdict:

- **approve** -- the PR is ready to merge as-is
- **request-changes** -- the PR needs revisions before it can merge
- **defer** -- the PR isn't ready for review right now (blocked, needs design input, etc.)

## revision

Pull request #{{PR_NUMBER}} received change requests during review.

Address the reviewer's feedback, update the code, and run tests. When the revisions are complete, submit `status: revised` to send the PR back for another review round.

## merge_prep

Pull request #{{PR_NUMBER}} has been approved. Proceed with merge preparation.

## parked

Pull request #{{PR_NUMBER}} has been deferred. No further action needed until it's unblocked.
