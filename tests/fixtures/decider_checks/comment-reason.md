---
name: decider-check-comment-reason
version: "1.0"
description: Demonstrates a decider check that grades the comments a change adds, in shadow.
initial_state: review
variables:
  BASE:
    description: Commit the change is compared against
    default: HEAD
states:
  review:
    gates:
      comment_reasons:
        type: decider-check
        # The code the change touched, with a little context, so each added
        # comment is read next to the code it describes.
        command: "git diff --unified=2 --no-color {{BASE}} -- ':(exclude)*.md'"
        label: change
        criteria:
          comment_reason:
            rule_ref: "https://github.com/tsukumogami/shirabe/blob/main/skills/work-on/references/phases/phase-4-implementation.md"
            question: "Does every comment this change adds record why the code is shaped this way, rather than restating what the code does?"
            pass: "Each added comment gives a reason, constraint, or rejected alternative that the code can't show."
            fail: "At least one added comment restates what the code does, or its reason is only a restatement."
            escape: "The change adds no comment, or the comments or the code are missing, so it can't be judged."
            mode: shadow
    transitions:
      - target: done
  done:
    terminal: true
---

## review

Check the comments your change adds.

## done

Done.
