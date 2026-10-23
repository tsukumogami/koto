---
name: koto-anchor-close
version: "1.0"
description: >
  The record that an anchor's agent session was observed to end, written by
  `koto session anchor close`. Its only state is terminal, so it reads
  finished everywhere.
initial_state: closed
states:
  closed:
    terminal: true
---

## closed

This session records that an anchor's agent session ended.
