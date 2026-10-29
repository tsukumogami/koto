---
name: compat-decider-routing
version: "1.0"
description: A routing decider consultation with no decider check, which koto v0.14.1 drives so the new build can read the log and ledger it wrote.
initial_state: work
variables:
  WORK_NOTE:
    description: What the routing decider reads at work
    default: "compat work note"
states:
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

## work

Keep working.

## done

Done.
