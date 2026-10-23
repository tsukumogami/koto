---
name: koto-anchor
version: "1.0"
description: >
  An agent session's anchor. koto opens it with `koto session anchor open`
  and keeps it for as long as the agent session exists. It never finishes,
  and `koto next` refuses it.
initial_state: open
states:
  open:
    transitions: []
---

## open

This session is koto's anchor for one agent session. Nothing ticks it.
