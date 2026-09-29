---
name: compat-decider-checks
version: "1.0"
description: A veto decider check the compat job fails, overrides, and leaves behind, then a shadow routing consultation.
initial_state: review
variables:
  WORK_NOTE:
    description: What the routing decider reads at work
    default: "compat work note"
states:
  review:
    gates:
      comments:
        type: decider-check
        command: "cat slice.txt"
        label: comments
        criteria:
          comment_reason:
            rule_ref: "https://example.org/rules/comment-reason"
            question: "Does every added comment give a reason rather than restate the code?"
            pass: "Each comment says why."
            fail: "A comment restates the code."
            escape: "No comment, or it can't be judged."
            mode: veto
    transitions:
      - target: work
  work:
    accepts:
      finished:
        type: boolean
        required: true
        description: Is the work finished?
        decider:
          answers:
            true: {description: "The work is finished."}
            false: {description: "More work remains."}
          inputs:
            - {var: WORK_NOTE, label: note}
    transitions:
      - target: done
        when:
          finished: true
  done:
    terminal: true
---

## review

Check the comments.

## work

Keep working.

## done

Done.
