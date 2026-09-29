---
name: value-routing-compat
version: "1.0"
description: Routes on MODE and ARM by value, and re-reads MODE after a rebind
initial_state: route
variables:
  MODE:
    description: Execution mode, re-applied on every attach
    values: [auto, interactive]
    default: interactive
    rebind: true
  ARM:
    description: Which arm of a split this run started in
    values: [with-rule, without-rule]
    default: without-rule
states:
  route:
    transitions:
      - target: arm_check
        when:
          vars.MODE: auto
      - target: work
        when:
          vars.MODE: interactive
  # The only edge is unconditional, so the ARM value on this transition's
  # vars_matched can come only from the skip_if map.
  arm_check:
    skip_if:
      vars.ARM: with-rule
    transitions:
      - target: work
  work:
    accepts:
      note:
        type: string
        required: true
    transitions:
      - target: reroute
        when:
          evidence.note: present
  reroute:
    transitions:
      - target: done
        when:
          vars.MODE: auto
      - target: review
        when:
          vars.MODE: interactive
  review:
    accepts:
      verdict:
        type: enum
        values: [approve, reject]
        required: true
    transitions:
      - target: done
        when:
          verdict: approve
      - target: work
        when:
          verdict: reject
  done:
    terminal: true
---

## route

Routes on MODE; the agent never sees this directive.

## arm_check

Skipped by ARM; the agent never sees this directive.

## work

Write a note about the work, then submit it as `note`.

## reroute

Routes on MODE again, after any rebind.

## review

Approve or reject the work.

## done

Done.
