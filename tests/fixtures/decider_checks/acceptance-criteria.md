---
name: decider-check-acceptance-criteria
version: "1.0"
description: Demonstrates a decider check that grades the acceptance criteria a change adds to its documents, in shadow.
initial_state: review
variables:
  BASE:
    description: Commit the change is compared against
    default: HEAD
states:
  review:
    gates:
      binary_criteria:
        type: decider-check
        # Only the acceptance-criterion lines the change adds, one per line.
        command: "git diff --unified=0 --no-color {{BASE}} -- '*.md' | grep -E '^[+] *- [[] []] ' | cut -c2-"
        label: acceptance_criteria
        criteria:
          ac_binary:
            rule_ref: "https://github.com/tsukumogami/shirabe/blob/main/skills/prd/references/prd-format.md"
            question: "Is every acceptance criterion listed binary pass/fail, verifiable by someone who didn't write it with no subjective judgment?"
            pass: "Each one names an observable condition that is either met or not."
            fail: "Checking at least one of them needs subjective judgment or an unstated threshold."
            escape: "The text is not a list of acceptance criteria at all."
            mode: shadow
    transitions:
      - target: done
  done:
    terminal: true
---

## review

Check the acceptance criteria your change adds.

## done

Done.
