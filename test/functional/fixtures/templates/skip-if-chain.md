---
name: skip-if-chain
version: "1.0"
description: 3-state chain where A and B auto-advance via skip_if when CHAIN_DISABLED is unset
initial_state: a

states:
  a:
    skip_if:
      vars.CHAIN_DISABLED:
        is_set: false
    transitions:
      - target: b
  b:
    skip_if:
      vars.CHAIN_DISABLED:
        is_set: false
    transitions:
      - target: c
  c:
    terminal: true
---

## a

Auto-advances to b when CHAIN_DISABLED is not set.

## b

Auto-advances to c when CHAIN_DISABLED is not set.

## c

Terminal state.
