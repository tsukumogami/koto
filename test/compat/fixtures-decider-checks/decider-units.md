---
name: compat-decider-units
version: "1.0"
description: A unit-mode veto decider check the compat job fails on one unit, hands one unit to the agent, delivers the rule for, overrides, and leaves behind.
initial_state: review
variables:
  WORK_NOTE:
    description: What the routing decider reads at work
    default: "compat work note"
states:
  review:
    gates:
      acceptance:
        type: decider-check
        command: "cat units.jsonl"
        label: ac
        max_bytes: 64
        units: jsonl
        unit_document: prd
        unit_section: acceptance-criteria
        criteria:
          ac_binary:
            rule_ref: "https://example.org/rules/ac-binary"
            question: "Can this acceptance criterion be checked pass or fail with no judgment?"
            pass: "It names one observable outcome a tester can check."
            fail: "It needs judgment, or names no observable outcome."
            escape: "It isn't an acceptance criterion."
            mode: veto
            summary: "State one observable outcome per criterion."
            rule_text: "Each acceptance criterion names one observable outcome a tester can check without judgment."
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

Check the acceptance criteria.

## work

Keep working.

## done

Done.
