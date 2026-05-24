---
name: audit-kind
version: "1.0"
description: Accepts an arbitrary `kind` discriminator so reserved-namespace rejection is observable end-to-end
initial_state: open

states:
  open:
    accepts:
      kind:
        type: string
        required: true
    transitions:
      - target: closed
  closed:
    terminal: true
---

## open

Submit evidence with any `kind` value. The runtime rejects values that
collide with the request-store reserved audit-event family.

## closed

Audit-kind workflow complete.
